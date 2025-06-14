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
        if "client_id" not in data or "pvn" not in data:
            return "Missing client_id or pvn field in data", 400
        return str(driver.initialize_pvn(data["client_id"], data["pvn"]))
    except driver.ValidationException as ve:
        return ve.message, 400


@api.route("/pvn/<id>", methods=["DELETE"])
def delete_pvn(id):
    driver.teardown_pvn(int(id))
    return ""
