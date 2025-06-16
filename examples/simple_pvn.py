#!/usr/bin/env python3

import sys
import requests

API_URL = "http://controller:11133/v1"

if len(sys.argv) != 2:
    print("Usage: ./simple_pvn <client_ip>")
    sys.exit(1)

pvn = {
    "apps": ["ethanvdh/ubuntu-full"],
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
        }
    ],
}

response = requests.post(f"{API_URL}/pvn", json={
    "client_ip": sys.argv[1],
    "pvn": pvn,
})
print("ID of new PVN:", response.text)
