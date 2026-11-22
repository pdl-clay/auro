# SPDX-License-Identifier: MIT OR Apache-2.0
"""Aggregate hot-reload dev traces into per-outcome latency statistics.

Usage: measure.py --input <jsonl> --platform <linux|macos|windows>
"""

import argparse
import json
import math
import statistics
import sys

TIMING_FIELDS = {
    "hot_reload": ("reloaded", "save_to_render_ms", (
        "save_to_patch_ms",
        "patch_to_render_ms",
        "save_to_render_ms",
        "patch_took_ms",
    )),
    "relaunch": ("restarted", "save_to_window_ms", ("save_to_window_ms",)),
}
OUTCOME_ORDER = ("reloaded", "restarted")


def _is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _format(value):
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return str(value)


def _fail(message, code):
    print(message, file=sys.stderr)
    return message, code


def aggregate(path, platform):
    """Return (groups, warmup_discarded) or an (error_message, exit_code) tuple."""
    groups = {outcome: [] for outcome in OUTCOME_ORDER}
    warmup_discarded = 0

    try:
        handle = open(path, "r", encoding="utf-8")
    except OSError as exc:
        return _fail(f"error: cannot read input {path}: {exc}", 2)

    with handle:
        for lineno, raw in enumerate(handle, 1):
            line = raw.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as exc:
                return _fail(f"error: line {lineno}: invalid JSON: {exc}", 2)
            if not isinstance(row, dict):
                return _fail(f"error: line {lineno}: expected a JSON object", 2)

            if row.get("warmup") is True:
                warmup_discarded += 1
                continue

            event = row.get("event")
            rule = TIMING_FIELDS.get(event)
            if rule is None:
                return _fail(f"error: line {lineno}: unknown event {event!r}", 2)
            outcome, timing_field, required_fields = rule

            for field in required_fields:
                if field not in row or row[field] is None or not _is_number(row[field]):
                    return _fail(
                        f"error: line {lineno}: event {event}: "
                        f"missing or null timing field {field}",
                        2,
                    )
            groups[outcome].append(row[timing_field])

    for outcome in OUTCOME_ORDER:
        values = groups[outcome]
        if values and len(values) < 10:
            return _fail(f"error: outcome {outcome}: N={len(values)} < 10", 3)

    results = []
    for outcome in OUTCOME_ORDER:
        values = groups[outcome]
        if not values:
            continue
        ordered = sorted(values)
        n = len(ordered)
        low, high = ordered[0], ordered[-1]
        results.append({
            "outcome": outcome,
            "n": n,
            "median_ms": statistics.median(ordered),
            "p95_ms": ordered[math.ceil(0.95 * n) - 1],
            "min_ms": low,
            "max_ms": high,
            "spread_ms": high - low,
        })
    return results, warmup_discarded


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Aggregate hot-reload dev traces into latency statistics."
    )
    parser.add_argument("--input", required=True, help="path to the JSONL trace")
    parser.add_argument(
        "--platform", required=True, choices=("linux", "macos", "windows")
    )
    args = parser.parse_args(argv)

    outcome = aggregate(args.input, args.platform)
    if isinstance(outcome[0], str):
        return outcome[1]

    results, warmup_discarded = outcome
    print("platform outcome N median_ms p95_ms spread_ms")
    for group in results:
        print(
            f"{args.platform} {group['outcome']} {group['n']} "
            f"{_format(group['median_ms'])} {_format(group['p95_ms'])} "
            f"{_format(group['spread_ms'])}"
        )
    summary = {
        "platform": args.platform,
        "groups": results,
        "warmup_discarded": warmup_discarded,
    }
    print("SUMMARY " + json.dumps(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
