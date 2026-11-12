"""Latency summaries use stage timestamps, including the first emit after focus."""

import importlib.machinery
import importlib.util
from pathlib import Path
import unittest

loader = importlib.machinery.SourceFileLoader(
    "pad_latency", str(Path(__file__).with_name("pad-latency"))
)
spec = importlib.util.spec_from_loader(loader.name, loader)
latency = importlib.util.module_from_spec(spec)
loader.exec_module(latency)


class LatencyTests(unittest.TestCase):
    def test_focus_samples_measure_event_age_separately_from_idle_time(self):
        lines = [
            'DEBUG pad_latency: stage="window_focus" at_us=100',
            'DEBUG pad_latency: stage="read" at_us=1000 physical_us=980',
            'DEBUG pad_latency: stage="emit" at_us=1010 read_us=1000 physical_us=980',
            'DEBUG pad_latency: stage="emit" at_us=1020 read_us=1010 physical_us=1000',
        ]
        self.assertEqual(latency.samples(lines), {
            "physical_to_read": [20],
            "physical_to_emit": [30, 20],
            "read_to_emit": [10, 10],
            "first_emit_after_focus": [30],
            "focus_to_first_emit": [910],
        })

    def test_invalid_unrelated_and_clock_reversed_samples_are_ignored(self):
        lines = [
            'DEBUG unrelated: stage="emit" at_us=200 read_us=100 physical_us=50',
            'DEBUG pad_latency: stage="emit" at_us=bad',
            'DEBUG pad_latency: stage="read" at_us=100 physical_us=200',
            '\x1b[34mDEBUG pad_latency:\x1b[0m stage="read" at_us=300 physical_us=250',
        ]
        self.assertEqual(latency.samples(lines), {"physical_to_read": [50]})

    def test_nearest_rank_percentiles(self):
        self.assertEqual(
            latency.report({"read_to_emit": list(range(1, 101))}),
            "stage count p50_us p95_us p99_us max_us\nread_to_emit 100 50 95 99 100",
        )


if __name__ == "__main__":
    unittest.main()
