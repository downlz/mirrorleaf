#!/usr/bin/env python3
"""The push-to-wake matrix (implementation plan §7.2, M4).

Sends test pushes to an Android phone through the ledger, in each phone state at
normal and high priority, and times what happens on the phone from its log:

  1. delivery: from FCM sending the push to the app receiving it;
  2. job start: from the app receiving the push to the sync job starting;
  3. sync: from the job starting to it finishing.

Each test push is one `pull` push sent with `mirrorleaf-ledger test-push`, which
skips the ledger's push rules and their 2-minute limit (the rules themselves are
tested in docs/tests/m4-android-tests.md). The app logs each push with FCM's
message ID, so late or duplicate pushes can't be mistaken for the current one.

Needs, on this Mac:
  - the ledger running with Firebase credentials (`docker compose up -d` in
    ledger/), and the phone registered with it and with a push token;
  - the phone connected with adb (USB, or wireless debugging for the states
    that need it unplugged; plugging in stops Doze, so the script simulates
    unplugging with `dumpsys battery unplug`);
  - ideally `mirrorleaf-cli sync --watch` running, so the job has a device to
    sync with.

Standard library only. Run from the repository root, for example:

  tools/push-matrix.py --device d57d4595 --states background,doze --trials 20

Writes every trial to a CSV file and prints a summary table for the test record.
"""

import argparse
import csv
import re
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

APP = "app.mirrorleaf"
ACTIVITY = f"{APP}/.MainActivity"
LOG_LINE = re.compile(r"^\s*(\d+\.\d+)\s+\d+\s+\d+\s+\w\s+mirrorleaf\s*:\s?(.*)$")
PUSH_LINE = re.compile(
    r"push: (\w+), (high|normal) priority \(sent as (high|normal)\), id=(\S+), sent at (\d+)"
)


@dataclass
class State:
    """A phone state from plan §7.2, and how the script sets it up."""

    title: str
    # adb shell commands run once before the state's trials.
    setup: list = field(default_factory=list)
    # adb shell commands run before every trial.
    each: list = field(default_factory=list)
    # adb shell commands that undo `setup`.
    teardown: list = field(default_factory=list)
    # Something the owner must do by hand before the trials start.
    manual: str = ""
    # Seconds to wait after setup before the first trial.
    settle: int = 0
    # Seconds to wait for each push before counting it as not delivered.
    timeout: int = 180
    # Whether the phone should look unplugged (needed for Doze and battery saver).
    unplugged: bool = False
    # Whether the screen is kept on (only for the "screen on" state).
    screen_on: bool = False


HOME = "input keyevent 3"
SCREEN_OFF = "input keyevent 223"
OPEN_APP = f"am start -n {ACTIVITY}"

STATES = {
    "background": State(
        "Screen on, app in the background",
        setup=[OPEN_APP, "sleep 4", HOME],
        screen_on=True,
        timeout=120,
    ),
    "screen-off": State(
        "Screen off for 10 minutes",
        setup=[OPEN_APP, "sleep 4", HOME, SCREEN_OFF],
        settle=600,
        unplugged=True,
        timeout=600,
    ),
    "doze": State(
        "Deep Doze",
        setup=[OPEN_APP, "sleep 4", HOME, SCREEN_OFF],
        each=[SCREEN_OFF, "dumpsys deviceidle force-idle"],
        teardown=["dumpsys deviceidle unforce"],
        unplugged=True,
        timeout=900,
    ),
    "swiped": State(
        "App swiped out of recents",
        manual="Open Mirrorleaf, then swipe it away in the recents screen.",
        # Later trials stand in for the swipe by killing the process the
        # previous push started, as Android does to a swiped-away app.
        each=[f"am kill {APP}"],
        timeout=180,
    ),
    "battery-saver": State(
        "Battery saver on",
        setup=[OPEN_APP, "sleep 4", HOME, "cmd power set-mode 1", SCREEN_OFF],
        teardown=["cmd power set-mode 0"],
        unplugged=True,
        timeout=600,
    ),
    "restricted": State(
        'App set to "restricted" battery use',
        setup=[
            OPEN_APP,
            "sleep 4",
            HOME,
            f"cmd appops set {APP} RUN_ANY_IN_BACKGROUND ignore",
            f"am set-standby-bucket {APP} restricted",
            SCREEN_OFF,
        ],
        teardown=[
            f"cmd appops set {APP} RUN_ANY_IN_BACKGROUND allow",
            f"am set-standby-bucket {APP} active",
        ],
        unplugged=True,
        timeout=600,
    ),
    "reboot": State(
        "After a reboot, app not opened",
        manual=(
            "The script will reboot the phone. When it is back, unlock it but "
            "don't open Mirrorleaf."
        ),
        setup=["reboot"],
        timeout=300,
    ),
    "force-stopped": State(
        "App force-stopped (expected to fail)",
        each=[f"am force-stop {APP}"],
        timeout=60,
    ),
}


@dataclass
class Trial:
    state: str
    priority: str
    number: int
    sent_ms: int = 0
    message_id: str = ""
    delivered: bool = False
    arrived_as: str = ""
    delivery_ms: int | None = None
    job_start_ms: int | None = None
    sync_ms: int | None = None
    outcome: str = ""


class Phone:
    def __init__(self, serial):
        self.adb = ["adb"] + (["-s", serial] if serial else [])

    def shell(self, command, check=False):
        if command.startswith("sleep "):
            time.sleep(float(command.split()[1]))
            return ""
        if command == "reboot":
            return self.reboot()
        result = subprocess.run(
            self.adb + ["shell", command], capture_output=True, text=True, check=check
        )
        return result.stdout.strip()

    def reboot(self):
        subprocess.run(self.adb + ["reboot"], check=True)
        subprocess.run(self.adb + ["wait-for-device"], check=True)
        while self.shell("getprop sys.boot_completed") != "1":
            time.sleep(2)
        input("  Phone restarted. Unlock it (don't open Mirrorleaf), then press Enter… ")
        return ""

    def clear_log(self):
        subprocess.run(self.adb + ["logcat", "-c"], check=False)

    def app_log(self):
        """The app's log lines since the last clear, as (Unix time in ms, text)."""
        out = subprocess.run(
            self.adb + ["logcat", "-d", "-v", "epoch", "-s", "mirrorleaf"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout
        lines = []
        for line in out.splitlines():
            m = LOG_LINE.match(line)
            if m:
                lines.append((int(float(m.group(1)) * 1000), m.group(2)))
        return lines


def send_push(compose, device, priority):
    """Sends one test push through the ledger; returns (sent ms, message ID)."""
    result = subprocess.run(
        ["docker", "compose", "-f", str(compose), "exec", "-T", "ledger"]
        + ["mirrorleaf-ledger", "test-push", device, priority],
        capture_output=True,
        text=True,
        check=False,
    )
    for line in result.stdout.splitlines():
        if line.startswith("sent "):
            _, sent, name = line.split(maxsplit=2)
            return int(sent), name.rsplit("/", 1)[-1]
    raise RuntimeError((result.stderr or result.stdout).strip() or "test-push failed")


def run_trial(phone, compose, device, state_key, state, priority, number):
    trial = Trial(state_key, priority, number)
    for command in state.each:
        phone.shell(command)
    phone.clear_log()
    try:
        trial.sent_ms, trial.message_id = send_push(compose, device, priority)
    except RuntimeError as e:
        trial.outcome = f"not sent: {e}"
        return trial
    deadline = time.time() + state.timeout
    received_at = job_at = None
    while time.time() < deadline:
        for at, text in phone.app_log():
            push = PUSH_LINE.search(text)
            if push and push.group(4) == trial.message_id and received_at is None:
                received_at = at
                trial.delivered = True
                trial.arrived_as = push.group(2)
                trial.delivery_ms = max(0, at - int(push.group(5)))
            elif received_at is not None and at >= received_at:
                if job_at is None and text.startswith("sync job started"):
                    job_at = at
                    trial.job_start_ms = at - received_at
                elif job_at is not None and text.startswith("sync job ("):
                    trial.sync_ms = at - job_at
                    trial.outcome = "synced" if "live=true" not in text else "app was open"
                    return trial
                elif job_at is not None and text.startswith("sync job failed"):
                    trial.sync_ms = at - job_at
                    trial.outcome = text
                    return trial
        time.sleep(1)
    if not trial.delivered:
        trial.outcome = f"not delivered within {state.timeout} s"
    elif job_at is None:
        trial.outcome = f"delivered, but no job started within {state.timeout} s"
    else:
        trial.outcome = f"job didn't finish within {state.timeout} s"
    return trial


def summary(trials):
    def stats(values):
        values = [v for v in values if v is not None]
        if not values:
            return "–"
        median = statistics.median(values) / 1000
        return f"{median:.1f} s (max {max(values) / 1000:.1f})"

    rows = [
        "| State | Priority | Delivered | Arrived as high | Delivery | Job start | Sync |",
        "|---|---|---|---|---|---|---|",
    ]
    seen = []
    for t in trials:
        if (t.state, t.priority) not in seen:
            seen.append((t.state, t.priority))
    for state_key, priority in seen:
        group = [t for t in trials if t.state == state_key and t.priority == priority]
        delivered = [t for t in group if t.delivered]
        as_high = (
            f"{sum(t.arrived_as == 'high' for t in delivered)}/{len(delivered)}"
            if priority == "high"
            else "–"
        )
        rows.append(
            f"| {STATES[state_key].title} | {priority} | {len(delivered)}/{len(group)} | "
            f"{as_high} | {stats([t.delivery_ms for t in group])} | "
            f"{stats([t.job_start_ms for t in group])} | {stats([t.sync_ms for t in group])} |"
        )
    return "\n".join(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--device", required=True, help="the phone's device ID (8+ hex digits)")
    parser.add_argument(
        "--states",
        default=",".join(STATES),
        help=f"comma-separated, from: {', '.join(STATES)} (default: all)",
    )
    parser.add_argument("--priorities", default="normal,high")
    parser.add_argument("--trials", type=int, default=20, help="per state and priority")
    parser.add_argument("--gap", type=int, default=20, help="seconds between trials")
    parser.add_argument("--timeout", type=int, help="seconds to wait per push (overrides the state's)")
    parser.add_argument("--serial", help="adb serial, if more than one phone is connected")
    parser.add_argument("--compose", default="ledger/compose.yaml", help="the ledger's compose file")
    parser.add_argument(
        "--out", help="CSV file (default: tools/push-matrix-results/<date and time>.csv)"
    )
    args = parser.parse_args()

    states = [s.strip() for s in args.states.split(",") if s.strip()]
    unknown = [s for s in states if s not in STATES]
    if unknown:
        sys.exit(f"unknown state(s): {', '.join(unknown)}")
    priorities = [p.strip() for p in args.priorities.split(",")]
    if any(p not in ("normal", "high") for p in priorities):
        sys.exit("priorities are normal and/or high")
    compose = Path(args.compose)
    if not compose.exists():
        sys.exit(f"{compose} not found: run from the repository root, or pass --compose")
    out = Path(args.out or f"tools/push-matrix-results/{datetime.now():%Y%m%d-%H%M}.csv")
    out.parent.mkdir(parents=True, exist_ok=True)

    phone = Phone(args.serial)
    if phone.shell("pm path " + APP) == "":
        sys.exit(f"{APP} isn't installed on the phone (or the phone isn't connected)")
    model = phone.shell("getprop ro.product.model")
    android = phone.shell("getprop ro.build.version.release")
    stay_on = phone.shell("settings get global stay_on_while_plugged_in") or "0"
    print(f"Phone: {model}, Android {android}. Results go to {out}.")

    trials = []
    try:
        for state_key in states:
            state = STATES[state_key]
            if args.timeout:
                state.timeout = args.timeout
            print(f"\n== {state.title}")
            phone.shell("svc power stayon true" if state.screen_on else "svc power stayon false")
            if state.unplugged:
                phone.shell("dumpsys battery unplug")
            if state.manual:
                input(f"  {state.manual} Press Enter when ready… ")
            for command in state.setup:
                phone.shell(command)
            if state.settle:
                print(f"  Waiting {state.settle} s before the first trial…")
                time.sleep(state.settle)
            try:
                for priority in priorities:
                    for number in range(1, args.trials + 1):
                        trial = run_trial(
                            phone, compose, args.device, state_key, state, priority, number
                        )
                        trials.append(trial)
                        timing = (
                            f"delivery {trial.delivery_ms} ms, job start {trial.job_start_ms} ms, "
                            f"sync {trial.sync_ms} ms"
                            if trial.delivered
                            else ""
                        )
                        print(f"  {priority} #{number}: {trial.outcome} {timing}".rstrip())
                        time.sleep(args.gap)
            finally:
                for command in state.teardown:
                    phone.shell(command)
                phone.shell("dumpsys battery reset")
    except KeyboardInterrupt:
        print("\nStopped. Writing the trials so far.")
    finally:
        phone.shell(f"settings put global stay_on_while_plugged_in {stay_on}")
        with out.open("w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(
                [
                    "state",
                    "priority",
                    "trial",
                    "sent_unix_ms",
                    "message_id",
                    "delivered",
                    "arrived_as",
                    "delivery_ms",
                    "job_start_ms",
                    "sync_ms",
                    "outcome",
                ]
            )
            for t in trials:
                writer.writerow(
                    [
                        t.state,
                        t.priority,
                        t.number,
                        t.sent_ms,
                        t.message_id,
                        t.delivered,
                        t.arrived_as,
                        t.delivery_ms,
                        t.job_start_ms,
                        t.sync_ms,
                        t.outcome,
                    ]
                )
    print(f"\n{len(trials)} trials written to {out}.\n")
    print(f"Phone: {model}, Android {android}\n")
    print(summary(trials))


if __name__ == "__main__":
    main()
