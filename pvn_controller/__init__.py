from flask import Flask

from pvn_controller.api import api
import pvn_controller.config as config


def create_app():
    app = Flask(__name__)
    app.register_blueprint(api)

    config.load_config()

    return app
