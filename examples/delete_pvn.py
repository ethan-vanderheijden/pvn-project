#!/usr/bin/env python3

import requests
import sys

API_URL = "http://controller:11133/v1"

if len(sys.argv) != 2:
    print("Usage: ./delete_pvn <pvn_id>")
    sys.exit(1)

pvn_id = sys.argv[1]
response = requests.delete(f"{API_URL}/pvn/{pvn_id}")
print(response.text)
