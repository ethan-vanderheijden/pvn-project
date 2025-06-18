#!/usr/bin/env python3

import sys
import requests

API_URL = "http://controller:11133/v1"

if len(sys.argv) != 2:
    print("Usage: ./long_pvn <client_ip>")
    sys.exit(1)

pvn = {
    "apps": ["ethanvdh/ubuntu-full", "ethanvdh/ubuntu-full"],
    "chains": [
        {
            "origin": -1,
            "edges": [
                {
                    "from": -1,
                    "to": 0,
                },
                {
                    "from": 0,
                    "to": 1,
                },
                {
                    "from": 1,
                    "to": 2,
                },
            ],
        },
        {
            "origin": 0,
            "edges": [
                {
                    "from": 0,
                    "to": 1,
                },
                {
                    "from": 1,
                    "to": 2,
                },
            ],
        },
        {
            "origin": 1,
            "edges": [
                {
                    "from": 1,
                    "to": 2,
                },
            ],
        },
        {
            "origin": 2,
            "edges": [
                {
                    "from": 2,
                    "to": 1,
                    "destination": -1,
                },
                {
                    "from": 1,
                    "to": 0,
                    "destination": -1,
                },
                {
                    "from": 0,
                    "to": -1,
                    "destination": -1,
                },
                {
                    "from": 2,
                    "to": 1,
                    "destination": 1,
                },
                {
                    "from": 2,
                    "to": 1,
                    "destination": 0,
                },
                {
                    "from": 1,
                    "to": 0,
                    "destination": 0,
                },
            ],
        },
    ],
}

response = requests.post(
    f"{API_URL}/pvn",
    json={
        "client_ip": sys.argv[1],
        "pvn": pvn,
    },
)

try:
    print("ID of new PVN:", int(response.text))
except ValueError:
    print(response.text)
