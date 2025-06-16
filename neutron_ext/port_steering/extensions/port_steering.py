import abc

from neutron_lib import exceptions as neutron_exc
from neutron_lib.plugins import directory
from neutron_lib.api import extensions as api_extensions
from neutron_lib.services import base as service_base
from neutron_lib import constants
from neutron_lib.db import constants as db_const
from oslo_config import cfg
from oslo_log import log

from neutron.api import extensions
from neutron.api.v2 import base
from neutron.common import config as common_config
from neutron.conf import service as service_config

from port_steering import extensions as port_extensions

LOG = log.getLogger(__name__)

common_config.register_common_config_options()
service_config.register_service_opts(service_config.SERVICE_OPTS, cfg.CONF)
cfg.CONF.import_opt("api_extensions_path", "neutron.common.config")
extensions.append_api_extensions_path(port_extensions.__path__)


SUPPORTED_ETHERTYPES = [constants.ETHERTYPE_IP, constants.ETHERTYPE_IPV6]


def normalize_ethertype(value):
    if value is None:
        return constants.ETHERTYPE_IP
    try:
        ether_type = int(value)
        if ether_type in SUPPORTED_ETHERTYPES:
            return ether_type
    except ValueError:
        pass

    raise UnsupportedEthertype(
        ethertype=value, values=SUPPORTED_ETHERTYPES)


PLUGIN_TYPE = "PORT_STEERING"

RESOURCE_NAME = "port_steering"
RESOURCE_ATTRIBUTE_MAP = {
    RESOURCE_NAME: {
        "id": {
            "allow_post": False,
            "allow_put": False,
            "is_visible": True,
            "validate": {"type:uuid": None},
            'is_filter': True,
            'is_sort_key': True,
            "primary_key": True,
        },
        "project_id": {
            "allow_post": True,
            "allow_put": False,
            "is_visible": True,
            "validate": {"type:string": db_const.PROJECT_ID_FIELD_SIZE},
            "required_by_policy": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "src_neutron_port": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:uuid_or_none": None},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "dest_neutron_port": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:uuid_or_none": None},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "src_ip": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:ip_address_or_none": None},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "dest_ip": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:ip_address_or_none": None},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "src_port": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:port_range": None},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "dest_port": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:port_range": None},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "ethertype": {
            "allow_post": True,
            "allow_put": False,
            "convert_to": normalize_ethertype,
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
        "protocol": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:range_or_none": [0, 255]},
            "default": None,
            "is_visible": True,
            'is_filter': True,
            'is_sort_key': True,
        },
    }
}


class PortSteeringNotFound(neutron_exc.NotFound):
    message = "Port Steering %(id)s not found."


class PortSteeringPortNotFound(neutron_exc.NotFound):
    message = "Port Steering Neutron Port %(id)s not found."


class UnsupportedEthertype(neutron_exc.InvalidInput):
    message = "Flow Classifier does not support ethertype %(ethertype)s. Supported ethertype values are %(values)s."


class Port_steering(api_extensions.ExtensionDescriptor):
    @classmethod
    def get_name(cls):
        return "port_steering"

    @classmethod
    def get_alias(cls):
        return "port_steering"

    @classmethod
    def get_description(cls):
        return "Steers egress packets from one port to another by changing destination MAC."

    @classmethod
    def get_updated(cls):
        return "2025-06-05T10:00:00-00:00"

    @classmethod
    def update_attributes_map(cls, extended_attributes, extension_attrs_map=None):
        super().update_attributes_map(
            extended_attributes, extension_attrs_map=RESOURCE_ATTRIBUTE_MAP
        )

    @classmethod
    def get_resources(cls):
        plugin = directory.get_plugin(PLUGIN_TYPE)
        params = RESOURCE_ATTRIBUTE_MAP.get(RESOURCE_NAME)
        collections_name = RESOURCE_NAME + "s"
        controller = base.create_resource(
            collections_name,
            RESOURCE_NAME,
            plugin,
            params,
            allow_bulk=True,
            allow_pagination=True,
            allow_sorting=True,
        )
        ext = extensions.ResourceExtension(collections_name, controller, attr_map=params)
        return [ext]

    @classmethod
    def get_plugin_interface(cls):
        return PortSteeringPluginBase


class PortSteeringPluginBase(service_base.ServicePluginBase, metaclass=abc.ABCMeta):
    @classmethod
    def get_plugin_type(cls):
        return PLUGIN_TYPE

    def get_plugin_description(self):
        return "Steers egress packets from one port to another by changing destination MAC."

    @abc.abstractmethod
    def get_port_steering(self, context, id, fields=None):
        pass

    @abc.abstractmethod
    def get_port_steerings(
        self,
        context,
        filters=None,
        fields=None,
        sorts=None,
        limit=None,
        marker=None,
        page_reverse=False,
    ):
        pass

    @abc.abstractmethod
    def create_port_steering(self, context, data):
        pass

    @abc.abstractmethod
    def update_port_steering(self, context, id, data):
        pass

    @abc.abstractmethod
    def delete_port_steering(self, context, id):
        pass
