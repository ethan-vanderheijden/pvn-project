import sqlalchemy as sa
from sqlalchemy import exc

import port_steering.extensions.port_steering as ext

from oslo_utils import uuidutils
from neutron.db import models_v2
from neutron_lib.db import model_query, utils as db_utils, api as db_api
from neutron_lib.db import model_base
from neutron_lib.db import constants as db_const

MAX_SELECTOR_LEN = 512


class PortSteering(model_base.BASEV2, model_base.HasId):
    __tablename__ = "port_steering"

    src_port = sa.Column(
        sa.String(db_const.UUID_FIELD_SIZE),
        sa.ForeignKey("ports.id", ondelete="CASCADE"),
        nullable=False,
    )
    dest_port = sa.Column(
        sa.String(db_const.UUID_FIELD_SIZE),
        sa.ForeignKey("ports.id", ondelete="CASCADE"),
        nullable=False,
    )
    flow_classifier = sa.Column(sa.String(512), nullable=True)

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
        src = port_steer["src_port"]
        dest = port_steer["dest_port"]
        with db_api.CONTEXT_WRITER.using(context):
            self._get_port(context, src)
            self._get_port(context, dest)

            port_steer_db = PortSteering(
                id=uuidutils.generate_uuid(),
                src_port=src,
                dest_port=dest,
                flow_classifier=data["flow_classifier"],
            )
            context.session.add(port_steer_db)
            return port_steer_db

    def _get_port(self, context, id):
        # raises an error if ports don't exist
        try:
            return model_query.get_by_id(context, models_v2.Port, id)
        except exc.NoResultFound as no_res_found:
            raise ext.PortSteeringPortNotFound(id=id) from no_res_found

    def _make_port_steering_dict(self, port_steering, fields=None):
        res = {
            "id": port_steering["id"],
            "src_port": port_steering["src_port"],
            "dest_port": port_steering["dest_port"],
            "flow_classifier": port_steering["flow_classifier"],
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

    def create_port_steering(self, context, data):
        result = self._create_port_steering(context, data)
        return self._make_port_steering_dict(result)

    def update_port_steering(self, context, id, data):
        new_steering = data["port_steering"]
        with db_api.CONTEXT_WRITER.using(context):
            old_steering = self._get_port_steering(context, id)
            old_steering.update(new_steering)
            return self._make_port_steering_dict(old_steering)

    def delete_port_steering(self, context, id):
        with db_api.CONTEXT_WRITER.using(context):
            steering = self._get_port_steering(context, id)
            context.session.delete(steering)
            return steering
