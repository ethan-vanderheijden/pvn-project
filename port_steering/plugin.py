import port_steering.extensions.port_steering as ext
from port_steering import model
from port_steering.rpc import PluginRpcServer, AgentRpcClient

from neutron_lib.db import model_query, api as db_api


class PortSteeringPlugin(model.PortSteeringDbPlugin):
    supported_extension_aliases = [ext.RESOURCE_NAME]

    __native_pagination_support = True
    __native_sorting_support = True
    __filter_validation_support = True

    def __init__(self):
        super().__init__()
        self.rpc_server = PluginRpcServer(self)
        self.notifier = AgentRpcClient()

    def create_port_steering(self, context, data):
        result = super()._create_port_steering(context, data)
        self.notifier.notify_steering_updated(context, result)
        return self._make_port_steering_dict(result)

    def delete_port_steering(self, context, id):
        steering = super().delete_port_steering(context, id)
        self.notifier.notify_steering_deleted(context, steering)

    def get_port_steering(self, context, ports):
        if not isinstance(ports, list):
            ports = [ports]
        with db_api.CONTEXT_READER.using(context):
            return model_query.get_collection(
                context, model.PortSteering, self._make_port_steering_dict, {"dest_port": ports}
            )
