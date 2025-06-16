import sys

from port_steering.rpc import AgentRpcServer, PluginRpcClient

from neutron_lib.agent import l2_extension
from neutron_lib.plugins.ml2 import ovs_constants
from oslo_log import log as logging
from oslo_config import cfg

LOG = logging.getLogger(__name__)

TARGET_TABLE = ovs_constants.ACCEPTED_EGRESS_TRAFFIC_NORMAL_TABLE
STEERING_PRIORITY = 100
DROP_PRIORITY = 99


class PortSteeringAgentExtension(l2_extension.L2AgentExtension):
    def initialize(self, connection, driver_type):
        if driver_type != ovs_constants.EXTENSION_DRIVER_TYPE:
            LOG.error(
                "Port steering extension is only supported for OVS, "
                "currently uses %(driver_type)s",
                {"driver_type": driver_type},
            )
            sys.exit(1)

        self.rpc_server = AgentRpcServer(self)
        self.plugin_client = PluginRpcClient(cfg.CONF.host)
        self.int_br = self.agent_api.request_int_br()

        self.steering_data = {}

    def consume_api(self, agent_api):
        self.agent_api = agent_api

    def handle_port(self, context, data):
        port_id = data["port_id"]
        if port_id not in self.steering_data:
            LOG.warn("Found new port.... " + str(data))
            steering_data = self.plugin_client.get_port_steering(context, [port_id])
            LOG.warn("Found steering data: " + str(steering_data))
            self.steering_data[port_id] = {rule["id"]: rule for rule in steering_data}
            self.steering_data[port_id]["ofport"] = data["vif_port"].ofport
            for rule in steering_data:
                self._install_rule(self._get_ofport(port_id), rule)

    def delete_port(self, context, data):
        port_id = data["port_id"]
        if port_id in self.steering_data:
            LOG.warn("Existing port was deleted.... " + str(data))
            ofport = self._get_ofport(port_id)
            data = self.steering_data.pop(port_id)
            data.pop("ofport")
            for rule in data.values():
                self._delete_rule(ofport, rule)
        else:
            LOG.warn("Untracked port was deleted.... ")

    def update_port_steering(self, context, **kwargs):
        steering_data = kwargs["port_steering"]
        rule_id = steering_data["id"]
        port_id = steering_data["src_neutron_port"]
        LOG.warn(f"Got update notification for {port_id}")
        if port_id in self.steering_data:
            LOG.warn("Updated steering data for tracked port")
            LOG.warn("Steering: " + str(steering_data))
            if rule_id in self.steering_data[port_id]:
                self._delete_rule(self._get_ofport(port_id), self.steering_data[port_id][rule_id])
            self.steering_data[port_id][rule_id] = steering_data
            self._install_rule(self._get_ofport(port_id), steering_data)

    def delete_port_steering(self, context, **kwargs):
        steering_data = kwargs["port_steering"]
        rule_id = steering_data["id"]
        port_id = steering_data["src_neutron_port"]
        LOG.warn(f"Got delete notification for {port_id}")
        if port_id in self.steering_data:
            if rule_id in self.steering_data[port_id]:
                rule = self.steering_data[port_id].pop(rule_id)
                self._delete_rule(self._get_ofport(port_id), rule)
                LOG.warn("Deleting steering data that was tracked")
            else:
                LOG.warn("Deleting untracked steering data for existing port")

    def _get_ofport(self, port_id):
        port_data = self.steering_data[port_id]
        if "target_ofport" not in port_data:
            port_data["target_ofport"] = self.int_br.get_vif_port_by_id(port_id).ofport
        LOG.warn("found ofport: " + str(port_data["target_ofport"]))
        return port_data["target_ofport"]

    def _prepare_matches(self, ofport, rule):
        if not rule.get("ethertype"):
            # If ethertype is not specified, build explicit rules for IPv4 and IPv6
            # avoids accidentally steering L2 packets (e.g. ARP)
            return [
                self._prepare_matches(ofport, {
                    **rule,
                    "ethertype": 0x0800,
                })[0],
                self._prepare_matches(ofport, {
                    **rule,
                    "ethertype": 0x86DD,
                })[0],
            ]

        match_kwargs = {}

        match_kwargs["in_port"] = ofport

        eth_type = rule["ethertype"]
        match_kwargs["eth_type"] = eth_type
        if eth_type == 0x0800:
            if rule.get("src_ip"):
                match_kwargs["ipv4_src"] = rule.get("src_ip")
            if rule.get("dest_ip"):
                match_kwargs["ipv4_dst"] = rule.get("dest_ip")
        elif eth_type == 0x86DD:
            if rule.get("src_ip"):
                match_kwargs["ipv6_src"] = rule.get("src_ip")
            if rule.get("dest_ip"):
                match_kwargs["ipv6_dst"] = rule.get("dest_ip")

        if rule.get("protocol"):
            proto = rule["protocol"]
            match_kwargs["ip_proto"] = proto
            if proto == 0x06:
                if rule.get("src_port"):
                    match_kwargs["tcp_src"] = rule.get("src_port")
                if rule.get("dest_port"):
                    match_kwargs["tcp_dst"] = rule.get("dest_port")
            elif proto == 0x11:
                if rule.get("src_port"):
                    match_kwargs["udp_src"] = rule.get("src_port")
                if rule.get("dest_port"):
                    match_kwargs["udp_dst"] = rule.get("dest_port")

        return [match_kwargs]

    def _install_rule(self, ofport, rule):
        (_, ofp, ofpp) = self.int_br._get_dp()
        if rule.get("overwrite_mac"):
            set_mac = ofpp.OFPActionSetField(eth_dst=rule["overwrite_mac"])
            normal = ofpp.OFPActionOutput(ofp.OFPP_NORMAL, 0)

            for match in self._prepare_matches(ofport, rule):
                self.int_br.install_apply_actions(
                    [set_mac, normal],
                    table_id=TARGET_TABLE,
                    priority=STEERING_PRIORITY,
                    **match,
                )
        else:
            for match in self._prepare_matches(ofport, rule):
                self.int_br.install_drop(
                    table_id=TARGET_TABLE,
                    priority=DROP_PRIORITY,
                    **match,
                )
        LOG.warn("Installed rule: " + str(rule))

    def _delete_rule(self, ofport, rule):
        priority = STEERING_PRIORITY
        if not rule.get("overwrite_mac"):
            priority = DROP_PRIORITY

        for match in self._prepare_matches(ofport, rule):
            self.int_br.uninstall_flows(
                strict=True,
                table_id=TARGET_TABLE,
                priority=priority,
                **match,
            )
        LOG.warn("Deleted rule: " + str(rule))
