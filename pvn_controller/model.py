from copy import deepcopy

# We will store data in-memory
# A better implementation would use an actual database

# maps PVN ID to PVN data
# PVN data includes:
#   - ip of end user
#   - status of PVN
#   - list of ports parallel to PVN app array
#   - list of PVN app container ids
#   - list of port steering rule ids
PVN_DB = {}

_NEXT_ID = 1


class PVNInvalidState(Exception):
    pass


class Status:
    INIT_PORTS = 1
    INIT_APPS = 2
    INIT_STEERING = 3
    ACTIVE = 4
    TEARING_DOWN = 5
    DELETED = 6


def get_pvn_by_client_ip(client_ip):
    for pvn in PVN_DB.values():
        if pvn["client_ip"] == client_ip and pvn["status"] != Status.DELETED:
            return deepcopy(pvn)
    return None


def get_pvn(pvn_id):
    data = PVN_DB.get(pvn_id)
    if data:
        return deepcopy(data)
    else:
        return None


def get_pvn_status(pvn_id):
    data = PVN_DB.get(pvn_id)
    if data:
        return data["status"]
    else:
        return None


def create_pvn(client_ip):
    global _NEXT_ID
    new_id = _NEXT_ID
    _NEXT_ID += 1

    PVN_DB[new_id] = {
        "client_ip": client_ip,
        "status": Status.INIT_PORTS,
        "ports": [],
        "apps": [],
        "steering": [],
    }

    return new_id


def set_ports(pvn_id, port_ids):
    PVN_DB[pvn_id]["ports"] = deepcopy(port_ids)
    if PVN_DB[pvn_id]["status"] != Status.INIT_PORTS:
        raise PVNInvalidState()
    PVN_DB[pvn_id]["status"] = Status.INIT_APPS


def set_apps(pvn_id, app_ids):
    PVN_DB[pvn_id]["apps"] = deepcopy(app_ids)
    if PVN_DB[pvn_id]["status"] != Status.INIT_APPS:
        raise PVNInvalidState()
    PVN_DB[pvn_id]["status"] = Status.INIT_STEERING


def set_steerings(pvn_id, steerings):
    PVN_DB[pvn_id]["steering"] = deepcopy(steerings)
    if PVN_DB[pvn_id]["status"] != Status.INIT_STEERING:
        raise PVNInvalidState()
    PVN_DB[pvn_id]["status"] = Status.ACTIVE


def teardown_pvn(pvn_id):
    PVN_DB[pvn_id]["status"] = Status.TEARING_DOWN


def delete_pvn(pvn_id):
    if PVN_DB[pvn_id]["status"] != Status.TEARING_DOWN:
        raise PVNInvalidState()

    PVN_DB[pvn_id]["status"] = Status.DELETED
