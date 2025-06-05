from port_steering.model import PortSteering
from port_steering.extensions import port_steering

from oslo_utils import uuidutils
from neutron.db import models_v2
from neutron_lib.db import model_query, utils as db_utils, api as db_api


class PortSteeringPlugin(port_steering.PortSteeringPluginBase):
    supported_extension_aliases = [port_steering.RESOURCE_NAME]

    __native_pagination_support = True
    __native_sorting_support = True
    __filter_validation_support = True

    def get_port_steering(self, context, id, fields=None):
        res = model_query.get_by_id(context, PortSteering, id)
        return db_utils.resource_fields(res.to_dict(), fields)

    def create_port_steering(self, context, data):
        port_steer = data["port_steering"]
        src = port_steer["src_port"]
        dest = port_steer["dest_port"]
        with db_api.CONTEXT_WRITER.using(context):
            # raises an error if ports don't exist
            model_query.get_by_id(context, models_v2.Port, src)
            model_query.get_by_id(context, models_v2.Port, dest)

            db_dict = {
                "id": uuidutils.generate_uuid(),
                "src_port": src,
                "dest_port": dest,
                "flow_classifier": data["flow_classifier"],
            }
            port_steer_db = PortSteering(**db_dict)
            context.session.add(port_steer_db)
            return db_dict
