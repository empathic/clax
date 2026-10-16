"""Each benchmark measure beside its budget, for the perf log.

The perf benchmarks (perf-daemon.py, perf-clients.py) call `metric` for
every measure they print, and `write` once at the end: with CLAX_PERF_JSON
set, each measure is appended to that file as one JSON line,
{"name", "value", "budget", "unit"}; without it nothing is written.
scripts/perf-log.sh collects the lines into one record per run.
"""

import json
import os

_METRICS = []


def metric(name, value, budget, unit="ms"):
    """Records one measure; `budget` is None where the measure has none."""
    def num(x):
        return None if x is None else round(float(x), 3)
    _METRICS.append({"name": name, "value": num(value), "budget": num(budget), "unit": unit})


def write(prefix):
    """Appends the recorded measures, each name after `prefix`, to CLAX_PERF_JSON."""
    out = os.environ.get("CLAX_PERF_JSON")
    if not out:
        return
    with open(out, "a", encoding="utf-8") as f:
        for m in _METRICS:
            f.write(json.dumps({**m, "name": prefix + m["name"]}) + "\n")
