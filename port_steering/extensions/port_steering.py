import abc

from neutron_lib import constants, exceptions as neutron_exc
from neutron_lib.plugins import directory
from neutron_lib.api import extensions as api_extensions
from neutron_lib.services import base as service_base
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

LOG.warn("EXECUTING!!! " + str(port_extensions.__path__))

PLUGIN_TYPE = "PORT_STEERING"

RESOURCE_NAME = "port_steering"
RESOURCE_ATTRIBUTE_MAP = {
    RESOURCE_NAME: {
        "id": {
            "allow_post": False,
            "allow_put": False,
            "is_visible": True,
            "validate": {"type:uuid": None},
            "primary_key": True,
        },
        "src_port": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:uuid": None},
            "default": constants.ATTR_NOT_SPECIFIED,
            "is_filter": True,
            "is_sort_key": True,
            "is_visible": True,
        },
        "dest_port": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:uuid": None},
            "default": constants.ATTR_NOT_SPECIFIED,
            "is_filter": True,
            "is_sort_key": True,
            "is_visible": True,
        },
        "flow_classifier": {
            "allow_post": True,
            "allow_put": False,
            "validate": {"type:string": None},
            "default": None,
            "is_filter": True,
            "is_visible": True,
        },
    }
}


class PortSteeringNotFound(neutron_exc.NotFound):
    message = "Port Steering %(id)s not found."


class PortSteeringPortNotFound(neutron_exc.NotFound):
    message = "Port Steering Neutron Port %(id)s not found."


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
