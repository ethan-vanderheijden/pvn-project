import sys

from keystoneauth1 import loading as ks_loading
from oslo_config import cfg
from zunclient import client as _zunclient
from neutronclient.v2_0 import client as _neutronclient

API_CONFIG_GROUP = "api"
NETWORK_CONFIG_GROUP = "network"
AUTH_CONFIG_GROUP = "keystone"

zun = None
neutron = None


def load_config():
    api_group = cfg.OptGroup(API_CONFIG_GROUP)
    api_opts = [
        cfg.StrOpt("host_ip"),
        cfg.PortOpt("port"),
    ]
    cfg.CONF.register_group(api_group)
    cfg.CONF.register_opts(api_opts, api_group)

    network_group = cfg.OptGroup(NETWORK_CONFIG_GROUP)
    network_opts = [
        cfg.StrOpt("id"),
        cfg.StrOpt("ingress_port"),
        cfg.StrOpt("egress_port"),
    ]
    cfg.CONF.register_group(network_group)
    cfg.CONF.register_opts(network_opts, network_group)

    auth_group = cfg.OptGroup(AUTH_CONFIG_GROUP)
    ks_loading.register_session_conf_options(cfg.CONF, auth_group)
    ks_loading.register_auth_conf_options(cfg.CONF, auth_group)
    ks_loading.register_adapter_conf_options(cfg.CONF, auth_group)

    cfg.CONF(sys.argv[1:])

    _auth = ks_loading.load_auth_from_conf_options(cfg.CONF, AUTH_CONFIG_GROUP)
    _sess = ks_loading.load_session_from_conf_options(cfg.CONF, AUTH_CONFIG_GROUP, auth=_auth)

    global zun, neutron
    zun = _zunclient.Client("1", session=_sess)
    neutron = _neutronclient.Client(session=_sess)
