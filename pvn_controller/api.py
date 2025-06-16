from flask import Blueprint, request

from pvn_controller import driver, model

api = Blueprint("api", __name__, url_prefix="/v1")


@api.route("/pvn/<id>", methods=["GET"])
def get_pvn(id):
    return model.get_pvn(int(id))


@api.route("/pvn", methods=["POST"])
def create_pvn():
    try:
        return str(driver.initialize_pvn(request.remote_addr, request.json))
    except driver.ValidationException as ve:
        return str(ve), 400


@api.route("/pvn/<id>", methods=["DELETE"])
def delete_pvn(id):
    driver.teardown_pvn(int(id))
    return ""
