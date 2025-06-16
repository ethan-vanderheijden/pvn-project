from flask import Blueprint, request

from pvn_controller import driver, model

api = Blueprint("api", __name__, url_prefix="/v1")


@api.route("/pvn/<id>", methods=["GET"])
def get_pvn(id):
    return model.get_pvn(int(id))


@api.route("/pvn", methods=["POST"])
def create_pvn():
    try:
        data = request.json
        if "client_ip" not in data or "pvn" not in data:
            return "client_ip or pvn field missing in request", 400
        return str(driver.initialize_pvn(data["client_ip"], data["pvn"]))
    except driver.ValidationException as ve:
        return str(ve), 400


@api.route("/pvn/<id>", methods=["DELETE"])
def delete_pvn(id):
    driver.teardown_pvn(int(id))
    return ""
