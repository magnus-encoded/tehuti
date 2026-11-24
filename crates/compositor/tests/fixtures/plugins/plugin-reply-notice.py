#!/usr/bin/env python3
import json
import sys


def send(message):
    print(json.dumps(dict(jsonrpc="2.0", **message)), flush=True)


for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        send(dict(id=message["id"], result=dict(
            protocol_version=0, encoding="json", events=["speech"])))
        send(dict(method="status", params=dict(text="Ada pending")))
    elif message.get("method") == "verb":
        send(dict(id=message["id"], result=dict(handled=True)))
        send(dict(method="notice", params=dict(text="Review complete")))
        send(dict(method="status", params=dict(text=None)))
