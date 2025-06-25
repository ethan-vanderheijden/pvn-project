from flask import Flask
from oslo_config import cfg

from controller.api import api
import controller.config as config


def _install_gateway_steering(gateway):
    steering = {
        "src_neutron_port": gateway,
        "dest_neutron_port": None,
    }
    current_steerings = config.neutron.get("/port_steerings", params=steering)
    if len(current_steerings["port_steerings"]) == 0:
        config.neutron.post(
            "/port_steerings",
            {
                "port_steering": steering,
            },
        )


def create_app():
    app = Flask(__name__)
    app.register_blueprint(api)

    config.load_config()

    _install_gateway_steering(cfg.CONF.network.ingress_port)
    _install_gateway_steering(cfg.CONF.network.egress_port)

    return app
