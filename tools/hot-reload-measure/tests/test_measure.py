# SPDX-License-Identifier: MIT OR Apache-2.0
"""Tests for the D6 hot-reload measurement aggregator.

These tests drive the real CLI via subprocess; expectations are hand-derived
literals and are never computed by importing the code under test.
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

MEASURE = Path(__file__).resolve().parents[1] / "measure.py"

RELOAD_VALUES = [100, 110, 120, 130, 140, 150, 160, 170, 180, 190]
RESTART_VALUES = [200, 210, 220, 230, 240, 250, 260, 270, 280, 290]


def reload_row(value, **overrides):
    row = {
        "event": "hot_reload",
        "save_to_patch_ms": 40,
        "patch_to_render_ms": value - 40,
        "save_to_render_ms": value,
        "patch_took_ms": 2,
    }
    row.update(overrides)
    return row


def restart_row(value, **overrides):
    row = {"event": "relaunch", "save_to_window_ms": value}
    row.update(overrides)
    return row


class MeasureCliTests(unittest.TestCase):
    """Exercise measure.py exactly as CI will invoke it."""

    def run_measure(self, rows, platform="linux"):
        handle = tempfile.NamedTemporaryFile(
            mode="w", suffix=".jsonl", delete=False, encoding="utf-8"
        )
        with handle:
            for row in rows:
                handle.write(json.dumps(row) + "\n")
        try:
            return subprocess.run(
                [
                    sys.executable,
                    str(MEASURE),
                    "--input",
                    handle.name,
                    "--platform",
                    platform,
                ],
                capture_output=True,
                text=True,
            )
        finally:
            os.unlink(handle.name)

    def summary(self, proc):
        lines = [line for line in proc.stdout.splitlines() if line.startswith("SUMMARY ")]
        self.assertEqual(len(lines), 1, proc.stdout)
        return json.loads(lines[0][len("SUMMARY ") :])

    def table_rows(self, proc):
        lines = proc.stdout.splitlines()
        self.assertEqual(lines[0], "platform outcome N median_ms p95_ms spread_ms")
        rows = [line for line in lines[1:] if not line.startswith("SUMMARY ")]
        return rows

    def test_ten_valid_samples_report_median_p95_and_spread(self):
        proc = self.run_measure([reload_row(value) for value in RELOAD_VALUES])
        self.assertEqual(proc.returncode, 0, proc.stderr)

        summary = self.summary(proc)
        self.assertEqual(summary["platform"], "linux")
        self.assertEqual(summary["warmup_discarded"], 0)
        self.assertEqual(len(summary["groups"]), 1)
        group = summary["groups"][0]
        self.assertEqual(group["outcome"], "reloaded")
        self.assertEqual(group["n"], 10)
        self.assertEqual(group["median_ms"], 145)
        self.assertEqual(group["p95_ms"], 190)
        self.assertEqual(group["min_ms"], 100)
        self.assertEqual(group["max_ms"], 190)
        self.assertEqual(group["spread_ms"], 90)

        rows = self.table_rows(proc)
        self.assertEqual(len(rows), 1)
        columns = rows[0].split()
        self.assertEqual(columns[:2], ["linux", "reloaded"])
        self.assertEqual(int(columns[2]), 10)
        self.assertEqual(float(columns[3]), 145)
        self.assertEqual(float(columns[4]), 190)
        self.assertEqual(float(columns[5]), 90)

    def test_null_stage_fails_and_names_the_stage(self):
        rows = [reload_row(value) for value in RELOAD_VALUES]
        rows[4]["patch_to_render_ms"] = None  # line 5 is the null stage
        proc = self.run_measure(rows)

        self.assertEqual(proc.returncode, 2)
        combined = proc.stdout + proc.stderr
        self.assertIn("line 5", combined)
        self.assertIn("hot_reload", combined)
        self.assertIn("patch_to_render_ms", combined)

    def test_nine_samples_fail(self):
        proc = self.run_measure([reload_row(value) for value in RELOAD_VALUES[:9]])

        self.assertEqual(proc.returncode, 3)
        combined = proc.stdout + proc.stderr
        self.assertIn("reloaded", combined)
        self.assertIn("N=9", combined)

    def test_empty_trace_fails_instead_of_reporting_success(self):
        proc = self.run_measure([])

        self.assertEqual(proc.returncode, 4)
        combined = proc.stdout + proc.stderr
        self.assertIn("no measurements", combined.lower())
        self.assertNotIn("SUMMARY ", proc.stdout)

    def test_mixed_outcomes_are_reported_separately(self):
        rows = [reload_row(value) for value in RELOAD_VALUES]
        rows += [restart_row(value) for value in RESTART_VALUES]
        proc = self.run_measure(rows)
        self.assertEqual(proc.returncode, 0, proc.stderr)

        summary = self.summary(proc)
        self.assertEqual(summary["warmup_discarded"], 0)
        groups = {group["outcome"]: group for group in summary["groups"]}
        self.assertEqual(set(groups), {"reloaded", "restarted"})

        reloaded = groups["reloaded"]
        self.assertEqual(reloaded["n"], 10)
        self.assertEqual(reloaded["median_ms"], 145)
        self.assertEqual(reloaded["p95_ms"], 190)
        self.assertEqual(reloaded["spread_ms"], 90)

        restarted = groups["restarted"]
        self.assertEqual(restarted["n"], 10)
        self.assertEqual(restarted["median_ms"], 245)
        self.assertEqual(restarted["p95_ms"], 290)
        self.assertEqual(restarted["min_ms"], 200)
        self.assertEqual(restarted["max_ms"], 290)
        self.assertEqual(restarted["spread_ms"], 90)

        rows = self.table_rows(proc)
        self.assertEqual(len(rows), 2)
        self.assertEqual({line.split()[1] for line in rows}, {"reloaded", "restarted"})

    def test_marked_warmups_are_discarded_and_counted(self):
        rows = [
            reload_row(9999, warmup=True, save_to_patch_ms=None),
            reload_row(9998, warmup=True, patch_took_ms=None),
        ]
        rows += [reload_row(value) for value in RELOAD_VALUES]
        proc = self.run_measure(rows)
        self.assertEqual(proc.returncode, 0, proc.stderr)

        summary = self.summary(proc)
        self.assertEqual(summary["warmup_discarded"], 2)
        self.assertEqual(len(summary["groups"]), 1)
        group = summary["groups"][0]
        self.assertEqual(group["n"], 10)
        self.assertEqual(group["median_ms"], 145)
        self.assertEqual(group["max_ms"], 190)


if __name__ == "__main__":
    unittest.main()
