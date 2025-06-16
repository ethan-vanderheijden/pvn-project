#!/usr/bin/env python3

import requests

API_URL = "http://controller:11133/v1"

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

response = requests.post(f"{API_URL}/pvn", json=pvn)
print(response.text)
