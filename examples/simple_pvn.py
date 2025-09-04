#!/usr/bin/env python3

import sys
import requests

API_URL = "http://controller:11133/v1"

if len(sys.argv) < 2:
    print('Usage: ./simple_pvn.py <client_ip> [image = "ethanvdh/ubuntu-full"] [args...]')
    sys.exit(1)

image = "ethanvdh/ubuntu-full"
if len(sys.argv) >= 3:
    image = sys.argv[2]

args = None
if len(sys.argv) > 3:
    args = sys.argv[3:]


app_description = image
if args:
    app_description = {
        "image": image,
        "args": args,
    }

pvn = {
    "apps": [app_description],
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
                    "to": 0,
                    "destination": -1,
                },
                {
                    "from": 1,
                    "to": 0,
                    "destination": 0,
                },
                {
                    "from": 0,
                    "to": -1,
                    "destination": -1,
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
