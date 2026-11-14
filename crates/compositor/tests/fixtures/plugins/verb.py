#!/usr/bin/env python3
"""A verb plugin: logs every line, replies with no actions, and swaps its verb after the first request."""
import json
import sys
from pathlib import Path

root = Path(__file__).parent
swapped = False
for line in sys.stdin:
    message = json.loads(line)
    with (root / 'received').open('a') as log:
        log.write(line)
    method = message.get('method')
    if method == 'initialize':
        result = {'protocol_version': 0, 'encoding': 'json', 'events': ['speech']}
    elif method == 'verb':
        accept = (root / 'accept').exists()
        result = [{'type': 'text', 'text': 'a'}, {'type': 'text', 'text': 'b'}] if accept else []
    else:
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)
    if method == 'verb' and not swapped and (root / 'swap').exists():
        swapped = True
        print(json.dumps({'jsonrpc': '2.0', 'method': 'verbs', 'params': {'verbs': ['orion']}}), flush=True)
        (root / 'swapped').write_text('yes')
    (root / 'replied').write_text('yes')
