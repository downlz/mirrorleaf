#!/usr/bin/env python3
"""Traces the Android app's memory over time (implementation plan §7.3).

Samples the app's main process every half second and follows its log, so each
sample can be put in a phase: in a push-woken job, screen open (live sync),
idle (cached after a job), or no process. Run it alongside anything that drives
the app, such as tools/push-matrix.py; stop it with Ctrl-C to get the summary.

Figures per sample, from /proc (read through `run-as`, so it needs a debuggable
build):
  - PSS: memory shared with other processes counted in proportion; the figure
    plan §7.3's targets use (`dumpsys meminfo` reports nearly the same).
  - PSS anon: the private, non-file part (heaps, stacks): what the code allocates.
  - PSS file: code and other files mapped in (the native library, dex).
  - RSS, the peak RSS since the process started (VmHWM), swap, and threads.

Only the app's main process is traced. The WebView renderer, which exists only
while the screen is open, is a separate process: measure it with
`dumpsys meminfo` while the app is on screen.

Standard library only. Example:

  tools/mem-trace.py --out tools/push-matrix-results/mem-trace

Writes <out>-samples.csv, <out>-events.csv and <out>-summary.md.
"""

import argparse
import csv
import re
import signal
import statistics
import subprocess
import sys
import threading
import time
from datetime import datetime
from pathlib import Path

LOG_LINE = re.compile(r"^\s*(\d+\.\d+)\s+(\d+)\s+\d+\s+\w\s+mirrorleaf\s*:\s?(.*)$")
FIELDS = ["Rss", "Pss", "Pss_Anon", "Pss_File", "Pss_Shmem", "Private_Dirty", "Swap", "SwapPss"]
EVENTS = [
    ("engine open", "engine open"),
    ("push:", "push received"),
    ("sync job started", "job started"),
    ("sync job (", "job finished"),
    ("sync job failed", "job failed"),
    ("live sync started", "screen open"),
    ("live sync stopped", "screen closed"),
]


class Tracer:
    def __init__(self, package, serial, interval):
        self.package = package
        self.adb = ["adb"] + (["-s", serial] if serial else [])
        self.interval = interval
        self.samples = []
        self.events = []
        self.stop = threading.Event()
        self.offset = self.clock_offset()

    def clock_offset(self):
        """Seconds to add to this Mac's clock to get the phone's (log times are the phone's)."""
        best = None
        for _ in range(5):
            before = time.time()
            out = subprocess.run(
                self.adb + ["shell", "date +%s.%N"], capture_output=True, text=True
            ).stdout.strip()
            after = time.time()
            try:
                offset = float(out) - (before + after) / 2
            except ValueError:
                continue
            if best is None or after - before < best[1]:
                best = (offset, after - before)
        return best[0] if best else 0.0

    def follow_log(self):
        """Streams the app's log. A live stream keeps every line even when
        another tool clears the log buffer."""
        proc = subprocess.Popen(
            self.adb + ["logcat", "-v", "epoch", "-T", "1", "-s", "mirrorleaf"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        while not self.stop.is_set():
            line = proc.stdout.readline()
            if not line:
                if proc.poll() is not None:
                    time.sleep(1)
                    return self.follow_log() if not self.stop.is_set() else None
                continue
            m = LOG_LINE.match(line)
            if not m:
                continue
            at, pid, text = float(m.group(1)), int(m.group(2)), m.group(3)
            for marker, kind in EVENTS:
                if marker in text:
                    self.events.append((at, pid, kind, text.strip()[:160]))
                    break
        proc.terminate()

    def sample(self):
        p = self.package
        script = (
            f"p=$(pidof {p}); if [ -n \"$p\" ]; then echo PID $p; "
            f"run-as {p} cat /proc/$p/smaps_rollup; "
            f"grep -E '^(VmHWM|Threads):' /proc/$p/status; else echo PID none; fi"
        )
        while not self.stop.is_set():
            started = time.time()
            out = subprocess.run(
                self.adb + ["shell", script], capture_output=True, text=True
            ).stdout
            row = {"time": started + self.offset, "pid": None}
            for line in out.splitlines():
                parts = line.replace(":", " ").split()
                if len(parts) >= 2 and parts[0] == "PID":
                    row["pid"] = None if parts[1] == "none" else int(parts[1])
                elif len(parts) >= 2 and parts[0] in FIELDS + ["VmHWM", "Threads"]:
                    try:
                        row[parts[0]] = int(parts[1])
                    except ValueError:
                        pass
            self.samples.append(row)
            self.stop.wait(max(0.0, self.interval - (time.time() - started)))

    def phase(self, row):
        """The phase a sample belongs to, from the log events before it."""
        if row["pid"] is None:
            return "no process"
        job = screen = False
        for at, pid, kind, _ in self.events:
            if at > row["time"]:
                break
            if pid != row["pid"]:
                continue
            if kind == "job started":
                job = True
            elif kind in ("job finished", "job failed"):
                job = False
            elif kind == "screen open":
                screen = True
            elif kind == "screen closed":
                screen = False
        if job:
            return "in a job"
        if screen:
            return "screen open"
        return "idle (cached)"


def mb(kb):
    return f"{kb / 1024:.1f}"


def summary(tracer, started, ended):
    tracer.events.sort()
    rows = [dict(r, phase=tracer.phase(r)) for r in tracer.samples]
    lines = [
        f"Memory trace of {tracer.package}, {datetime.fromtimestamp(started):%d %b %Y %H:%M} to "
        f"{datetime.fromtimestamp(ended):%H:%M} ({len(rows)} samples every {tracer.interval} s).",
        "",
        "| Phase | Samples | PSS median | PSS max | PSS anon median | PSS anon max | PSS file median | RSS max | Swap median | Threads median |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for phase in ("in a job", "idle (cached)", "screen open", "no process"):
        group = [r for r in rows if r["phase"] == phase]
        if not group:
            continue
        if phase == "no process":
            lines.append(f"| {phase} | {len(group)} | – | – | – | – | – | – | – | – |")
            continue

        def col(key, fn):
            values = [r[key] for r in group if key in r]
            return mb(fn(values)) if values else "–"

        threads = [r["Threads"] for r in group if "Threads" in r]
        lines.append(
            f"| {phase} | {len(group)} | {col('Pss', statistics.median)} | {col('Pss', max)} | "
            f"{col('Pss_Anon', statistics.median)} | {col('Pss_Anon', max)} | "
            f"{col('Pss_File', statistics.median)} | {col('Rss', max)} | "
            f"{col('Swap', statistics.median)} | "
            f"{int(statistics.median(threads)) if threads else '–'} |"
        )
    lines.append("")
    lines.append("All figures in MB (1 MB = 1024 KB).")

    # One line per process: how it started and its peak RSS (VmHWM catches
    # peaks between samples).
    pids = []
    for r in rows:
        if r["pid"] is not None and r["pid"] not in pids:
            pids.append(r["pid"])
    if pids:
        lines += ["", "| Process | Started by | Jobs | Peak RSS (VmHWM) | Max PSS |", "|---|---|---|---|---|"]
    for pid in pids:
        group = [r for r in rows if r["pid"] == pid]
        kinds = [k for _, p, k, _ in tracer.events if p == pid]
        first = next((k for k in kinds if k in ("push received", "screen open")), "?")
        started_by = {"push received": "a push", "screen open": "opening the app"}.get(first, "?")
        hwm = max((r.get("VmHWM", 0) for r in group), default=0)
        pss = max((r.get("Pss", 0) for r in group), default=0)
        jobs = kinds.count("job started")
        lines.append(f"| {pid} | {started_by} | {jobs} | {mb(hwm)} | {mb(pss)} |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--package", default="app.mirrorleaf")
    parser.add_argument("--serial", help="adb serial, if more than one phone is connected")
    parser.add_argument("--interval", type=float, default=0.5, help="seconds between samples")
    parser.add_argument(
        "--out",
        help="file prefix (default: tools/push-matrix-results/mem-<date and time>)",
    )
    args = parser.parse_args()
    out = Path(args.out or f"tools/push-matrix-results/mem-{datetime.now():%Y%m%d-%H%M}")
    out.parent.mkdir(parents=True, exist_ok=True)

    tracer = Tracer(args.package, args.serial, args.interval)
    started = time.time()
    print(
        f"Tracing {args.package} every {args.interval} s (phone clock offset "
        f"{tracer.offset:+.2f} s). Ctrl-C to stop.",
        flush=True,
    )
    signal.signal(signal.SIGTERM, lambda *_: tracer.stop.set())
    threads = [
        threading.Thread(target=tracer.follow_log, daemon=True),
        threading.Thread(target=tracer.sample, daemon=True),
    ]
    for t in threads:
        t.start()
    try:
        while not tracer.stop.is_set():
            tracer.stop.wait(1)
    except KeyboardInterrupt:
        tracer.stop.set()
    ended = time.time()
    time.sleep(0.5)

    with open(f"{out}-samples.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["phone_time", "pid", "phase"] + FIELDS + ["VmHWM", "Threads"])
        for r in tracer.samples:
            w.writerow(
                [f"{r['time']:.3f}", r["pid"] or "", tracer.phase(r)]
                + [r.get(k, "") for k in FIELDS + ["VmHWM", "Threads"]]
            )
    with open(f"{out}-events.csv", "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["phone_time", "pid", "event", "log"])
        for at, pid, kind, text in sorted(tracer.events):
            w.writerow([f"{at:.3f}", pid, kind, text])
    text = summary(tracer, started, ended)
    Path(f"{out}-summary.md").write_text(text + "\n")
    print(text)
    print(f"\nWritten: {out}-samples.csv, -events.csv, -summary.md")


if __name__ == "__main__":
    sys.exit(main())
