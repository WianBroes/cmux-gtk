#!/usr/bin/env python3
"""Prove `cmux settings` and `cmux shortcuts` land on the one Preferences window the app has.

Only what is observable is judged: the application runs in isolated storage under openbox, and the checks look at
the visible windows it owns through the window manager. This port has no separate Shortcuts page, so both verbs must
open the same single Preferences window rather than a second one. Every check prints the raw value it judged.
"""
import json
from pathlib import Path
import subprocess
import tempfile

from linux_app import running_app


def windows(app, name=None):
    """List the visible windows owned by this fixture, optionally matching one exact title."""
    arguments = ["xdotool", "search", "--all", "--onlyvisible", "--pid", str(app.process.pid)]
    if name:
        arguments.extend(["--name", "^" + name + "$"])
    result = subprocess.run(arguments, capture_output=True, text=True, timeout=5)
    return result.stdout.split() if result.returncode == 0 else []


def main():
    """Run the two verbs and judge the Preferences window each one leaves behind."""
    results = []
    with tempfile.TemporaryDirectory(prefix="cmux-settings-window-") as directory:
        root = Path(directory)
        wm = subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            with running_app(root) as app:
                app.wait_for(lambda: bool(windows(app)), "visible main window")

                def check(name, condition, seen):
                    """Record one judged outcome with the value that decided it."""
                    results.append((name, bool(condition), seen))
                    print(("PASS " if condition else "FAIL ") + name, "|", seen, flush=True)

                def verb(*arguments):
                    """Run one production CLI invocation and return its parsed JSON answer."""
                    printed = app.cli(*arguments, timeout=20)
                    print("  cmux", *arguments, "->", printed.strip()[:160], flush=True)
                    return json.loads(printed)

                check("no Preferences window before the command", windows(app, "Preferences") == [],
                      windows(app, "Preferences"))

                opened = verb("settings", "--json")
                check("cmux settings reports the window as opened", opened.get("opened") is True, opened)
                app.wait_for(lambda: bool(windows(app, "Preferences")), "Preferences window after cmux settings")
                first = windows(app, "Preferences")
                check("cmux settings leaves exactly one Preferences window", len(first) == 1, first)

                reopened = verb("shortcuts", "--json")
                check("cmux shortcuts reports the window as opened", reopened.get("opened") is True, reopened)
                app.wait_for(lambda: bool(windows(app, "Preferences")), "Preferences window after cmux shortcuts")
                second = windows(app, "Preferences")
                check("cmux shortcuts leaves exactly one Preferences window, not a second", len(second) == 1,
                      f"before {first} after {second}")
        finally:
            wm.terminate()
            wm.wait(timeout=10)

    failed = [name for name, ok, _ in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} settings window checks passed", flush=True)
    if failed:
        raise AssertionError("settings window wrong: " + "; ".join(failed))
    print("cmux settings and cmux shortcuts opened the single Preferences window through the real window manager")


if __name__ == "__main__":
    main()
