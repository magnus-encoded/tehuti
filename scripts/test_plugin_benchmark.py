import importlib.util
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name('plugin-benchmark.py')


class PluginBenchmarkGateTests(unittest.TestCase):
    def test_json_breach_fails_and_reopens_encoding_issue(self):
        spec = importlib.util.spec_from_file_location('plugin_benchmark', SCRIPT)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        calls = []
        results = {'button': {'round_trip_ns': 2_000_000, 'json_share_percent': 26},
                   'axis': {'round_trip_ns': 900_000, 'json_share_percent': 10}}
        with tempfile.TemporaryDirectory() as directory:
            summary = pathlib.Path(directory) / 'summary.md'
            status = gate.check(results, summary, lambda args: calls.append(args) or '')
            self.assertEqual(status, 1)
            self.assertIn('JSON share exceeds', summary.read_text())
            self.assertIn('25', summary.read_text())
            self.assertIn('26', summary.read_text())
        self.assertEqual(calls[0], ['gh', 'issue', 'reopen', '24'])
        self.assertEqual(calls[1][:4], ['gh', 'issue', 'comment', '24'])
        self.assertIn('2000000', calls[1][-1])

    def test_slow_transport_fails_and_opens_triage_issue(self):
        spec = importlib.util.spec_from_file_location('plugin_benchmark', SCRIPT)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        calls = []
        results = {'button': {'round_trip_ns': 1_000_001, 'json_share_percent': 25},
                   'axis': {'round_trip_ns': 900_000, 'json_share_percent': 10}}
        with tempfile.TemporaryDirectory() as directory:
            summary = pathlib.Path(directory) / 'summary.md'
            self.assertEqual(gate.check(results, summary, lambda args: calls.append(args) or ''), 1)
            self.assertIn('fast path in the core', summary.read_text())
        self.assertEqual(calls[0][:3], ['gh', 'issue', 'create'])
        self.assertIn('needs-triage', calls[0])
        self.assertIn('1000001', calls[0][-1])

    def test_p99_uses_paired_json_share_and_excludes_the_slowest_one_percent(self):
        spec = importlib.util.spec_from_file_location('plugin_benchmark', SCRIPT)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        samples = [{'round_trip_ns': 1000, 'json_ns': 200}] * 98
        samples += [{'round_trip_ns': 2000, 'json_ns': 100},
                    {'round_trip_ns': 9000, 'json_ns': 9000}]
        self.assertEqual(gate.summarize({'button': samples, 'axis': samples}), {
            'button': {'round_trip_ns': 2000, 'json_ns': 200, 'json_share_percent': 20.0},
            'axis': {'round_trip_ns': 2000, 'json_ns': 200, 'json_share_percent': 20.0}})

    def test_exact_thresholds_pass_without_touching_issues(self):
        spec = importlib.util.spec_from_file_location('plugin_benchmark', SCRIPT)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        calls = []
        results = {name: {'round_trip_ns': 1_000_000, 'json_share_percent': 25}
                   for name in ('button', 'axis')}
        with tempfile.TemporaryDirectory() as directory:
            summary = pathlib.Path(directory) / 'summary.md'
            self.assertEqual(gate.check(results, summary, lambda args: calls.append(args)), 0)
        self.assertEqual(calls, [])

    def test_mixed_breaches_take_both_actions(self):
        spec = importlib.util.spec_from_file_location('plugin_benchmark', SCRIPT)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        calls = []
        results = {'button': {'round_trip_ns': 500_000, 'json_share_percent': 26},
                   'axis': {'round_trip_ns': 1_000_001, 'json_share_percent': 10}}
        with tempfile.TemporaryDirectory() as directory:
            status = gate.check(results, pathlib.Path(directory) / 'summary.md',
                                lambda args: calls.append(args) or '')
        self.assertEqual(status, 1)
        self.assertEqual([call[2] for call in calls], ['reopen', 'comment', 'create'])

    def test_invalid_or_incomplete_samples_are_rejected(self):
        spec = importlib.util.spec_from_file_location('plugin_benchmark', SCRIPT)
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)
        for samples in ({}, {'button': [], 'axis': []},
                        {'button': [{'round_trip_ns': 0, 'json_ns': 0}], 'axis': []},
                        {'button': [{'round_trip_ns': 1, 'json_ns': 2}], 'axis': []}):
            with self.subTest(samples=samples), self.assertRaises(ValueError):
                gate.summarize(samples)

    def test_cli_fails_for_each_threshold_and_uses_only_gh_for_issue_writes(self):
        import json
        import os
        import subprocess
        import sys

        for total, codec, commands in ((1000, 251, ['reopen', 'comment']),
                                       (1_000_001, 100, ['create'])):
            with self.subTest(total=total, codec=codec), tempfile.TemporaryDirectory() as directory:
                directory = pathlib.Path(directory)
                log = directory / 'gh.jsonl'
                gh = directory / 'gh'
                gh.write_text('#!/usr/bin/env python3\nimport json, os, sys\n'
                              'assert os.environ["GH_TOKEN"] == "workflow-token"\n'
                              'with open(os.environ["GH_LOG"], "a") as log:\n'
                              '    log.write(json.dumps(sys.argv[1:]) + "\\n")\n')
                gh.chmod(0o755)
                results = directory / 'results.json'
                results.write_text(json.dumps({name: [{'round_trip_ns': total, 'json_ns': codec}]
                                               for name in ('button', 'axis')}))
                summary = directory / 'summary.md'
                env = dict(os.environ, PATH=f'{directory}:{os.environ["PATH"]}',
                           GH_TOKEN='workflow-token', GH_LOG=str(log),
                           GITHUB_STEP_SUMMARY=str(summary))
                process = subprocess.run([sys.executable, str(SCRIPT), str(results)], env=env,
                                         capture_output=True, text=True, check=False)
                self.assertEqual(process.returncode, 1, process.stderr)
                calls = [json.loads(line) for line in log.read_text().splitlines()]
                self.assertEqual([call[1] for call in calls], commands)
                self.assertIn(str(total), summary.read_text())
                self.assertIn('Thresholds:', summary.read_text())
