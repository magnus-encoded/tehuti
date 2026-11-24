#!/usr/bin/env python3
import json
import sys
from pathlib import Path

root = Path(__file__).parent
with (root / "launches").open("a") as launches:
    launches.write("started\n")

for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": 0, "result": {
            "protocol_version": 0, "encoding": "json", "events": ["button"]
        }}), flush=True)
        (root / "replied").write_text("yes")
    elif message.get("method") == "button":
        continue
