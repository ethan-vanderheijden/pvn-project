#!/usr/bin/env python3

import sys
import requests

API_URL = "http://controller:11133/v1"

if len(sys.argv) < 2 or len(sys.argv) > 3:
    print('Usage: ./out_of_band_pvn <client_ip> [image = "ethanvdh/ubuntu-full"]')
    sys.exit(1)

image = "ethanvdh/ubuntu-full"
if len(sys.argv) == 3:
    image = sys.argv[2]

pvn = {
    "apps": [image],
    "chains": [
        {
            "origin": -1,
            "edges": [
                {
                    "from": -1,
                    "to": 1,
                },
                {
                    "from": -1,
                    "to": 0,
                    "destination": 0,
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
                    "from": 0,
                    "to": -1,
                    "destination": -1,
                },
            ],
        },
        {
            "origin": 1,
            "edges": [
                {
                    "from": 1,
                    "to": -1,
                    "destination": -1,
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
