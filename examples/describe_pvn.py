#!/usr/bin/env python3

import requests
import sys
import json

API_URL = "http://controller:11133/v1"

if len(sys.argv) != 2:
    print("Usage: ./describe_pvn <pvn_id>")
    sys.exit(1)

pvn_id = sys.argv[1]
response = requests.get(f"{API_URL}/pvn/{pvn_id}")

try:
    print(json.dumps(response.json(), indent=2))
except Exception:
    print(response.text)
