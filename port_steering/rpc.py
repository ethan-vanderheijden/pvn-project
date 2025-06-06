import oslo_messaging
from neutron.agent import rpc as agent_rpc
from neutron_lib import rpc as n_rpc
from neutron_lib.agent import topics

PLUGIN_TOPIC = "q-port-steering-plugin"
AGENT_TOPIC = "q-port-steering-agent"
STEERING_TABLE = "steering"


class PluginRpcServer():
    def __init__(self, api):
        self.api = api
        conn = n_rpc.Connection()
        conn.create_consumer(PLUGIN_TOPIC, [self], fanout=False)
        conn.consume_in_threads()

    def get_port_steering(self, context, **kwargs):
        ports = kwargs.get("ports")
        if ports:
            return self.api.get_port_steering(context, ports)
        else:
            return []


class PluginRpcClient():
    def __init__(self, host):
        self.host = host
        target = oslo_messaging.Target(topic=PLUGIN_TOPIC, version="1.0")
        self.client = n_rpc.get_client(target)

    def get_port_steering(self, context, ports):
        cctxt = self.client.prepare()
        return cctxt.call(
            context,
            'get_port_steering',
            ports=ports,
        )


class AgentRpcServer():
    def __init__(self, api):
        agent_rpc.create_consumers(
            [api],
            AGENT_TOPIC,
            [
                [STEERING_TABLE, topics.CREATE],
                [STEERING_TABLE, topics.DELETE],
            ],
            start_listening=True
        )


class AgentRpcClient():
    def __init__(self):
        self.topic = AGENT_TOPIC
        target = oslo_messaging.Target(topic=self.topic)
        self.client = n_rpc.get_client(target)

    def notify_steering_updated(self, context, port_steering):
        cctxt = self.client.prepare(
            topic=topics.get_topic_name(self.topic, STEERING_TABLE, topics.CREATE),
            fanout=True,
        )
        cctxt.cast(context, "update_port_steering", port_steering=port_steering)

    def notify_steering_deleted(self, context, port_steering):
        cctxt = self.client.prepare(
            topic=topics.get_topic_name(self.topic, STEERING_TABLE, topics.DELETE),
            fanout=True,
        )
        cctxt.cast(context, "delete_port_steering", port_steering=port_steering)
