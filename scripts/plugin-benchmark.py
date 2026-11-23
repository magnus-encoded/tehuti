#!/usr/bin/env python3
"""Report the plugin benchmark and route threshold breaches through gh."""

import argparse
import json
import math
import os
from pathlib import Path
import subprocess


THRESHOLDS = Path(__file__).with_name('plugin-benchmark-thresholds.json')

SLOW_TITLE = 'Plugin round trip exceeds its budget'


def run(args):
    return subprocess.check_output(args, text=True).strip()


def summarize(workloads):
    if set(workloads) != {'button', 'axis'}:
        raise ValueError('benchmark requires button and axis samples')
    results = {}
    for name, samples in workloads.items():
        if not samples:
            raise ValueError(f'{name}: no samples')
        for sample in samples:
            total, codec = sample['round_trip_ns'], sample['json_ns']
            if (type(total) is not int or type(codec) is not int
                    or total <= 0 or not 0 <= codec <= total):
                raise ValueError(f'{name}: invalid timing sample')
        rank = math.ceil(len(samples) * 99 / 100) - 1
        results[name] = {
            'round_trip_ns': sorted(sample['round_trip_ns'] for sample in samples)[rank],
            'json_ns': sorted(sample['json_ns'] for sample in samples)[rank],
            'json_share_percent': sorted(100 * sample['json_ns'] / sample['round_trip_ns']
                                         for sample in samples)[rank],
        }
    return results


def check(results, summary, gh=run):
    thresholds = json.loads(THRESHOLDS.read_text())
    report = ['## Plugin stdio benchmark (p99)', '',
              '| Workload | Round trip (ns) | JSON (ns) | JSON share (%) |',
              '| --- | ---: | ---: | ---: |']
    for name, result in results.items():
        report.append(f"| {name} | {result['round_trip_ns']} | {result.get('json_ns', 'n/a')} | "
                      f"{result['json_share_percent']} |")
    report.extend(['', f"Thresholds: round trip > {thresholds['round_trip_ns']} ns; "
                   f"JSON share > {thresholds['json_share_percent']}%."])
    json_breach = any(value['json_share_percent'] > thresholds['json_share_percent']
                      for value in results.values())
    slow = any(value['round_trip_ns'] > thresholds['round_trip_ns']
               and value['json_share_percent'] <= thresholds['json_share_percent']
               for value in results.values())
    if slow:
        report.extend(['', 'Round trip exceeds the threshold: fix the fast path in the core; '
                       'a binary encoding would not help.'])
    if json_breach:
        report.extend(['', 'JSON share exceeds the threshold: reopen #24 for binary encoding.'])
    text = '\n'.join(report) + '\n'
    with summary.open('a') as output:
        output.write(text)
    print(text, end='')
    if json_breach:
        gh(['gh', 'issue', 'reopen', '24'])
        gh(['gh', 'issue', 'comment', '24', '--body', text])
    if slow:
        existing = gh(['gh', 'issue', 'list', '--state', 'open', '--label', 'needs-triage',
                       '--search', f'"{SLOW_TITLE}" in:title', '--json', 'number',
                       '--jq', '.[0].number // empty'])
        if existing:
            gh(['gh', 'issue', 'comment', existing, '--body', text])
        else:
            gh(['gh', 'issue', 'create', '--title', SLOW_TITLE,
                '--label', 'needs-triage', '--body', text])
    return int(json_breach or slow)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('results', type=Path)
    args = parser.parse_args()
    return check(summarize(json.loads(args.results.read_text())),
                 Path(os.environ.get('GITHUB_STEP_SUMMARY', '/dev/stdout')))


if __name__ == '__main__':
    raise SystemExit(main())
