import sqlalchemy as sa

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
