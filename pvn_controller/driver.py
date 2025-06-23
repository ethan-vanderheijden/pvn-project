import jsonschema
from eventlet.greenthread import spawn, sleep
from oslo_config import cfg

from pvn_controller import model, config

PVN_SCHEMA = {
    "title": "PVN",
    "description": "A representation of a PVN service chain.",
    "type": "object",
    "properties": {
        "apps": {
            "description": "Applications to be instantiated along the service chain.",
            "type": "array",
            "items": {
                "type": "string",
            },
            "minItems": 1,
        },
        "chains": {
            "description": "Traffic steering chains (a DAG) to be established between apps.",
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "origin": {
                        "description": "Traffic origin of chain (an index into 'apps' array; -1 for end user and 'apps.length' for egress gateway).",
                        "type": "number",
                        "minimum": -1,
                    },
                    "edges": {
                        "description": "Directed edges representing steering rules between apps.",
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "from": {
                                    "type": "number",
                                    "minimum": -1,
                                },
                                "to": {
                                    "type": "number",
                                    "minimum": -1,
                                },
                                "destination": {
                                    "type": "number",
                                    "minimum": -1,
                                },
                                "protocol": {
                                    "type": "number",
                                    "minimum": 0,
                                    "maximum": 255,
                                },
                                "source_port": {
                                    "type": "number",
                                    "minimum": 1,
                                    "maximum": 65535,
                                },
                                "destination_port": {
                                    "type": "number",
                                    "minimum": 1,
                                    "maximum": 65535,
                                },
                            },
                            "required": ["from", "to"],
                            "dependentSchemas": {
                                "source_port": {
                                    "properties": {
                                        "protocol": {
                                            "type": "number",
                                            "enum": [0x06, 0x11],
                                        },
                                    },
                                    "required": ["protocol"],
                                },
                                "destination_port": {
                                    "properties": {
                                        "protocol": {
                                            "type": "number",
                                            "enum": [0x06, 0x11],
                                        },
                                    },
                                    "required": ["protocol"],
                                },
                            },
                        },
                        "minItems": 1,
                    },
                },
                "required": ["origin"],
            },
            "minItems": 1,
        },
    },
    "required": ["apps", "chains"],
}


class ValidationException(Exception):
    pass


def initialize_pvn(client_ip, ethertype, pvn_json):
    try:
        jsonschema.validate(pvn_json, PVN_SCHEMA)
    except jsonschema.ValidationError as ve:
        raise ValidationException("Failed to validate JSON schema: " + ve.message)

    existing_origins = set()
    max_app_index = len(pvn_json["apps"])
    for chain in pvn_json["chains"]:
        if chain["origin"] in existing_origins:
            raise ValidationException(
                f"Can't have multiple app chains with same origin of {chain["origin"]}."
            )
        existing_origins.add(chain["origin"])

        if chain["origin"] > max_app_index:
            raise ValidationException(f"Chain with origin {chain["origin"]} is invalid app index.")
        for edge in chain["edges"]:
            if edge["from"] > max_app_index:
                raise ValidationException(
                    f"Chain with origin {chain["origin"]} has edge with invalid from index: {edge["from"]}."
                )
            if edge["to"] > max_app_index:
                raise ValidationException(
                    f"Chain with origin {chain["origin"]} has edge with invalid to index: {edge["to"]}."
                )
            if "destination" in edge and edge["destination"] >= max_app_index:
                raise ValidationException(
                    f"Chain with origin {chain["origin"]} has edge with invalid destination specifier: {edge["destination"]}."
                )

            if chain["origin"] == max_app_index and "destination" not in edge:
                raise ValidationException(
                    "Chain with origin at egress gateway must specify destination classifier on each edge."
                )

    if -1 not in existing_origins:
        raise ValidationException(
            "Must have an app chain with an origin at the end user (i.e. origin of -1)."
        )

    for chain in pvn_json["chains"]:
        visited_edges = [False for _ in chain["edges"]]
        if not _is_single_origin_dag(chain["origin"], set(), chain["edges"], visited_edges):
            raise ValidationException(f"Chain with origin {chain["origin"]} is not a DAG.")
        if not all(visited_edges):
            raise ValidationException(
                f"Some edge in chain with origin {chain["origin"]} will never be traversed."
            )

    if model.get_pvn_by_client_ip(client_ip):
        raise ValidationException("A PVN for this source IP address already exists.")

    # TODO: there is a bug inside OpenStack's zun library that throws an
    # error when searching for an image name with a slash in it
    # _validate_images(pvn_json["apps"])

    pvn_id = model.create_pvn(client_ip, ethertype)
    spawn(_start_pvn, client_ip, ethertype, pvn_id, pvn_json)

    return pvn_id


def teardown_pvn(pvn_id, force=False):
    status = model.get_pvn_status(pvn_id)
    if status is None or status == model.Status.DELETED:
        return

    model.teardown_pvn(pvn_id)
    if not force and status != model.Status.ACTIVE:
        # PVN is still booting up. Initialization process will error out and call
        # teardown_pvn again when it's ready
        return

    pvn = model.get_pvn(pvn_id)
    for steering in pvn["steering"]:
        spawn(_delete_steering, steering)
    for app in pvn["apps"]:
        spawn(_stop_container, app)
    for port in pvn["ports"]:
        spawn(_delete_port, port)
    model.delete_pvn(pvn_id)


def _is_single_origin_dag(start, visited_nodes, edges, visited_edges):
    if start in visited_nodes:
        return False

    visited_nodes.add(start)

    for i, edge in enumerate(edges):
        if edge["from"] == start:
            visited_edges[i] = True
            if not _is_single_origin_dag(edge["to"], visited_nodes.copy(), edges, visited_edges):
                return False
    return True


def _validate_images(images):
    for image in images:
        candidates = config.zun.images.search_image(image, exact_match=True)
        if len(candidates) == 0:
            raise ValidationException(f"Image for application {image} does not exist.")
        elif len(candidates) > 1:
            raise ValidationException(
                f"Application {image} is ambiguous as there are multiple available images."
            )


def _create_ports(pvn_id, count, network):
    body = {"ports": []}
    for i in range(0, count):
        body["ports"].append(
            {
                "name": f"pvn.{pvn_id}.app.{i}",
                "network_id": network,
            }
        )
    print("SAW THISSSS")
    result = config.neutron.create_port(body)
    # TODO: support multiple ip address per port (e.g. IPv6 and IPv4)
    print(result)
    return [(port["id"], port["fixed_ips"][0]["ip_address"]) for port in result["ports"]]


def _delete_port(port_id):
    config.neutron.delete_port(port_id)


def _create_container(port, image, *args):
    result = config.zun.containers.run(image=image, nets=[{"port": port}], command=args, auto_remove=True)
    uuid = result.uuid
    for i in range(0, 20):
        status = config.zun.containers.get(uuid).status.lower()
        print("Curr status:", status)
        if status != "creating" and status != "created":
            return uuid
        sleep(0.1)
    raise Exception("Container failed to start.")


def _stop_container(container_id):
    config.zun.containers.stop(container_id, 3)


def _prepare_steering(chain_origin, client_ip, ethertype, ports, edge):
    def index_to_port(index):
        if index == -1:
            return (cfg.CONF.network.ingress_port, client_ip)
        elif index == len(ports):
            return (cfg.CONF.network.egress_port, None)
        else:
            return ports[index]

    src_neutron = index_to_port(edge["from"])[0]
    dest_neutron = index_to_port(edge["to"])[0]
    steering = {
        "src_neutron_port": src_neutron,
        "dest_neutron_port": dest_neutron,
    }
    if ethertype == 4:
        steering["ethertype"] = 0x0800
    elif ethertype == 6:
        steering["ethertype"] = 0x86DD
    if "destination" in edge:
        steering["dest_ip"] = index_to_port(edge["destination"])[1]
    if "protocol" in edge:
        steering["protocol"] = edge["protocol"]
    if "source_port" in edge:
        steering["src_port"] = edge["source_port"]
    if "destination_port" in edge:
        steering["dest_port"] = edge["destination_port"]

    steering["src_ip"] = index_to_port(chain_origin)[1]

    return steering


def _create_steerings(steerings):
    body = {"port_steerings": steerings}
    result = config.neutron.post("/port_steerings", body=body)
    return [steering["id"] for steering in result["port_steerings"]]


def _delete_steering(steering_id):
    config.neutron.delete(f"/port_steerings/{steering_id}")


def _start_pvn(client_ip, ethertype, pvn_id, pvn_json):
    try:
        ports = _create_ports(pvn_id, len(pvn_json["apps"]), cfg.CONF.network.id)
        model.set_ports(pvn_id, [port[0] for port in ports])

        app_threads = []
        for i, app in enumerate(pvn_json["apps"]):
            app_threads.append(spawn(_create_container, ports[i][0], app, client_ip))
        app_ids = [thread.wait() for thread in app_threads]
        model.set_apps(pvn_id, app_ids)

        steerings = []
        for chain in pvn_json["chains"]:
            origin = chain["origin"]
            for edge in chain["edges"]:
                steerings.append(_prepare_steering(origin, client_ip, ethertype, ports, edge))

        for port in ports:
            steerings.append({"src_neutron_port": port[0]})

        steering_ids = _create_steerings(steerings)
        model.set_steerings(pvn_id, steering_ids)
    except Exception:
        teardown_pvn(pvn_id, True)
        raise
