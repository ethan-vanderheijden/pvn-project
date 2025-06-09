import sqlalchemy as sa
from sqlalchemy import exc

import port_steering.extensions.port_steering as ext

from oslo_utils import uuidutils
from neutron.db import models_v2
from neutron_lib.db import model_query, utils as db_utils, api as db_api
from neutron_lib.db import model_base
from neutron_lib.db import constants as db_const

MAX_SELECTOR_LEN = 512


class PortSteering(model_base.BASEV2, model_base.HasId, model_base.HasProject):
    __tablename__ = "port_steering"

    src_neutron_port = sa.Column(
        sa.String(db_const.UUID_FIELD_SIZE),
        sa.ForeignKey("ports.id", ondelete="CASCADE"),
        nullable=False,
    )
    dest_neutron_port = sa.Column(
        sa.String(db_const.UUID_FIELD_SIZE),
        sa.ForeignKey("ports.id", ondelete="CASCADE"),
        nullable=False,
    )
    src_ip = sa.Column(
        sa.String(db_const.IP_ADDR_FIELD_SIZE),
        nullable=True,
    )
    dest_ip = sa.Column(
        sa.String(db_const.IP_ADDR_FIELD_SIZE),
        nullable=True,
    )
    src_port = sa.Column(
        sa.Integer(),
        nullable=True,
    )
    dest_port = sa.Column(
        sa.Integer(),
        nullable=True,
    )
    ethertype = sa.Column(
        sa.Integer(),
        nullable=True,
    )
    protocol = sa.Column(
        sa.Integer(),
        nullable=True,
    )

    api_collections = []
    collection_resource_map = {}


class PortSteeringDbPlugin(ext.PortSteeringPluginBase):
    def _get_port_steering(self, context, id):
        try:
            return model_query.get_by_id(context, PortSteering, id)
        except exc.NoResultFound as no_res_found:
            raise ext.PortSteeringNotFound(id=id) from no_res_found

    def _create_port_steering(self, context, data):
        port_steer = data["port_steering"]
        src_neutron = port_steer["src_neutron_port"]
        dest_neutron = port_steer["dest_neutron_port"]
        with db_api.CONTEXT_WRITER.using(context):
            self._get_neutron_port(context, src_neutron)
            self._get_neutron_port(context, dest_neutron)

            port_steer_db = PortSteering(
                id=uuidutils.generate_uuid(),
                src_neutron_port=src_neutron,
                dest_neutron_port=dest_neutron,
                src_ip=port_steer.get("src_ip"),
                dest_ip=port_steer.get("dest_ip"),
                src_port=port_steer.get("src_port"),
                dest_port=port_steer.get("dest_port"),
                ethertype=port_steer.get("ethertype"),
                protocol=port_steer.get("protocol"),
            )
            context.session.add(port_steer_db)
            return port_steer_db

    def _get_neutron_port(self, context, id):
        # raises an error if ports don't exist
        try:
            return model_query.get_by_id(context, models_v2.Port, id)
        except exc.NoResultFound as no_res_found:
            raise ext.PortSteeringPortNotFound(id=id) from no_res_found

    def _make_port_steering_dict(self, port_steering, fields=None):
        res = {
            "id": port_steering["id"],
            "src_neutron_port": port_steering["src_neutron_port"],
            "dest_neutron_port": port_steering["dest_neutron_port"],
            "src_ip": port_steering.get("src_ip"),
            "dest_ip": port_steering.get("dest_ip"),
            "src_port": port_steering.get("src_port"),
            "dest_port": port_steering.get("dest_port"),
            "ethertype": port_steering.get("ethertype"),
            "protocol": port_steering.get("protocol"),
        }
        return db_utils.resource_fields(res, fields)

    @db_api.CONTEXT_READER
    def get_port_steering(self, context, id, fields=None):
        res = self._get_port_steering(context, id)
        return self._make_port_steering_dict(res, fields=fields)

    @db_api.CONTEXT_READER
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
        marker_obj = db_utils.get_marker_obj(self, context, ext.RESOURCE_NAME, limit, marker)
        return model_query.get_collection(
            context,
            PortSteering,
            self._make_port_steering_dict,
            filters=filters,
            fields=fields,
            sorts=sorts,
            limit=limit,
            marker_obj=marker_obj,
            page_reverse=page_reverse,
        )

    def create_port_steering(self, context, port_steering):
        result = self._create_port_steering(context, port_steering)
        return self._make_port_steering_dict(result)

    def update_port_steering(self, context, id, port_steering):
        new_steering = port_steering["port_steering"]
        with db_api.CONTEXT_WRITER.using(context):
            old_steering = self._get_port_steering(context, id)
            old_steering.update(new_steering)
            return self._make_port_steering_dict(old_steering)

    def delete_port_steering(self, context, id):
        with db_api.CONTEXT_WRITER.using(context):
            steering = self._get_port_steering(context, id)
            context.session.delete(steering)
            return steering
