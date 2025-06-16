import port_steering.extensions.port_steering as ext
from port_steering import model
from port_steering.rpc import PluginRpcServer, AgentRpcClient

from neutron.db import models_v2
from neutron_lib.db import model_query, api as db_api
from oslo_log import log as logging

LOG = logging.getLogger(__name__)


class PortSteeringPlugin(model.PortSteeringDbPlugin):
    supported_extension_aliases = [ext.RESOURCE_NAME]

    __native_pagination_support = True
    __native_sorting_support = True
    __filter_validation_support = True

    def __init__(self):
        super().__init__()
        self.rpc_server = PluginRpcServer(self)
        self.notifier = AgentRpcClient()

    def create_port_steering(self, context, port_steering):
        steering = super().create_port_steering(context, port_steering)

        dest_mac = None
        if steering.get("dest_neutron_port"):
            dest_port = model_query.get_by_id(
                context, models_v2.Port, steering["dest_neutron_port"]
            )
            dest_mac = dest_port.mac_address
        self.notifier.notify_steering_updated(context, {
            **steering,
            "overwrite_mac": dest_mac,
        })

        return steering

    def delete_port_steering(self, context, id):
        steering = super().delete_port_steering(context, id)
        self.notifier.notify_steering_deleted(context, steering)

    def get_steering_info(self, context, ports):
        with db_api.CONTEXT_READER.using(context):
            data = model_query.get_collection(
                context,
                model.PortSteering,
                self._make_port_steering_dict,
                {"src_neutron_port": ports},
            )
            for steering in data:
                if steering.get("dest_neutron_port"):
                    dest_port = model_query.get_by_id(
                        context, models_v2.Port, steering["dest_neutron_port"]
                    )
                    steering["overwrite_mac"] = dest_port.mac_address
                else:
                    steering["overwrite_mac"] = None
            return data
