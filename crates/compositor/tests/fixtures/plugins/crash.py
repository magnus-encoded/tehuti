#!/usr/bin/env python3
import json
import sys
from pathlib import Path

root = Path(__file__).parent
with (root / "launches").open("a") as launches:
    launches.write("started\n")
sys.exit(7)
