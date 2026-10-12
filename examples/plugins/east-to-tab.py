#!/usr/bin/env python3
"""Remap pressed East to Tab for the manifest's matching application."""

import json
import sys


def reply(request_id, result):
    print(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}),
        flush=True,
    )


def main():
    initialized = False
    for line in sys.stdin:
        message = json.loads(line)
        method = message.get("method")
        params = message.get("params", {})
        if method == "initialize":
            if params.get("protocol_version") != 0 or params.get("encoding") != "json":
                print("Unsupported protocol version or encoding", file=sys.stderr)
                return
            reply(
                message["id"],
                {"protocol_version": 0, "encoding": "json", "events": ["button"]},
            )
            initialized = True
        elif method == "button" and initialized:
            actions = []
            if params.get("name") == "east" and params.get("state") == "pressed":
                actions.append({"type": "key", "key": "tab"})
            reply(message["id"], actions)


if __name__ == "__main__":
    main()
