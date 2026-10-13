#!/usr/bin/env python3
import json
import sys
import time
from pathlib import Path

root = Path(__file__).parent
with (root / "launches").open("a") as launches:
    launches.write(str(time.monotonic_ns()) + "\n")
sys.exit(7)
