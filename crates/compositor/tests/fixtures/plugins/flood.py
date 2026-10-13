#!/usr/bin/env python3
import json
import sys
import time
from pathlib import Path

root = Path(__file__).parent
with (root / "launches").open("a") as launches:
    launches.write(str(time.monotonic_ns()) + "\n")

for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": 0, "result": {
            "protocol_version": 0, "encoding": "json", "events": ["button"]
        }}), flush=True)
        (root / "replied").write_text("yes")
    elif message.get("method") == "button":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": []}), flush=True)
        wire = (root / "wire").read_text()
        chunk = wire * 128
        while True:
            sys.stdout.write(chunk)
            sys.stdout.flush()
