#!/usr/bin/env python3
import json
import sys
from pathlib import Path

root = Path(__file__).parent


def send(**message):
    print(json.dumps(dict(jsonrpc="2.0", **message)), flush=True)


for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        send(id=message["id"], result=dict(
            protocol_version=0, encoding="json", events=["speech"]))
    elif message.get("method") == "verb":
        command = message["params"]["text"]
        if command == "crash":
            sys.exit(1)
        if command == "flood":
            for _ in range(8192):
                send(method="status", params=dict(text="Work pending"))
        if command == "malformed_burst":
            for _ in range(16):
                send(method="status", params=dict(text="bad\nstatus"))
        if command == "set":
            send(method="status", params=dict(text="Work pending"))
        elif command == "clear":
            send(method="status", params=dict(text=None))
        elif command == "malformed":
            send(method="status", params=dict(text="bad\nstatus"))
            send(method="status", params=dict(text="x" * 257))
            send(method="notice", params=dict(text="bad\x00notice"))
        elif command == "notice":
            send(method="notice", params=dict(text="Work complete"))
        send(id=message["id"], result=dict(handled=True))
        with (root / "processed").open("a") as log:
            log.write(command + "\n")
