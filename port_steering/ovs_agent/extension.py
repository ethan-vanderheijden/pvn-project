import sys

from port_steering.rpc import AgentRpcServer, PluginRpcClient

from neutron_lib.agent import l2_extension
from neutron_lib.plugins.ml2 import ovs_constants
from oslo_log import log as logging
from oslo_config import cfg

LOG = logging.getLogger(__name__)


class PortSteeringAgentExtension(l2_extension.L2AgentExtension):
    def initialize(self, connection, driver_type):
        if driver_type != ovs_constants.EXTENSION_DRIVER_TYPE:
            LOG.error('Port steering extension is only supported for OVS, '
                      'currently uses %(driver_type)s',
                      {'driver_type': driver_type})
            sys.exit(1)

        self.rpc_server = AgentRpcServer(self)
        self.plugin_client = PluginRpcClient(cfg.CONF.host)

        self.steering_data = {}

    def consume_api(self, agent_api):
        self.agent_api = agent_api

    def handle_port(self, context, data):
        port_id = data["port_id"]
        if port_id in self.steering_data:
            LOG.warn("Existing port was changed...")
        else:
            LOG.warn("Found new port.... " + str(data))
            steering_data = self.plugin_client.get_port_steering(context, [port_id])
            LOG.warn("Found steering data: " + str(steering_data))
            self.steering_data[port_id] = steering_data

    def delete_port(self, context, data):
        port_id = data["port_id"]
        if port_id in self.steering_data:
            LOG.warn("Existing port was deleted.... " + str(data))
            del self.steering_data[port_id]
        else:
            LOG.warn("Untracked port was deleted.... ")

    def update_port_steering(self, context, **kwargs):
        steering_data = kwargs["port_steering"]
        port_id = steering_data["src_neutron_port"]
        LOG.warn(f"Got update notification for {port_id}")
        if port_id in self.steering_data:
            LOG.warn("Updated steering data for tracked port")

    def delete_port_steering(self, context, **kwargs):
        steering_data = kwargs["port_steering"]
        port_id = steering_data["src_neutron_port"]
        LOG.warn(f"Got delete notification for {port_id}")
        if port_id in self.steering_data:
            if steering_data["id"] in self.steering_data[port_id]:
                LOG.warn("Deleting steering data that was tracked")
            else:
                LOG.warn("Deleting untracked teering data for existing port")
