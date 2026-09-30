#!/usr/bin/env python3
"""Prove a configured shortcut works in the main window after a live reload, and the old key stops working.

Real X11 key events go through the window manager to the running application. The default split-right key
(Ctrl+D) first splits the pane; `cmux.json` then rebinds `splitRight` to Ctrl+Shift+K and `cmux reload-config`
applies it without a restart. Afterwards the new key splits, the old key does nothing, and typed text (an
unbound input) still reaches the terminal. Text is typed before the old key is pressed so that a Ctrl+D that
reaches the shell is harmless (on a non-empty line bash does not exit).
"""
import json
from pathlib import Path
import subprocess
import tempfile
import time

from linux_app import running_app


def main():
    """Default key, live rebinding, old key inert, typed text delivered to the terminal."""
    with tempfile.TemporaryDirectory(prefix="cmux-shortcut-") as directory:
        root = Path(directory)
        wm = subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            with running_app(root) as app:
                app.wait_for(lambda: len(app.children()) == 1, "initial shell")
                windows = subprocess.check_output(
                    ["xdotool", "search", "--onlyvisible", "--pid", str(app.process.pid)], text=True, timeout=10,
                ).split()
                assert windows, "no visible main window"

                def panes():
                    """Count terminal surfaces (one per pane) through the production CLI."""
                    return len(app.surfaces())

                def press(chord):
                    """Deliver a real key chord to the focused main window through the window manager."""
                    subprocess.check_call(
                        ["xdotool", "windowfocus", "--sync", windows[-1], "key", "--clearmodifiers", chord], timeout=10,
                    )

                def active_text():
                    """Viewport text of the active terminal."""
                    surface = next(row["uuid"] for row in app.surfaces() if row["active"])
                    return json.loads(app.cli("read-text", "--id", surface, "--json"))["text"]

                assert panes() == 1
                app.wait_for(lambda: bool(active_text().strip()), "first prompt")
                press("ctrl+d")
                app.wait_for(lambda: panes() == 2, "default Ctrl+D splitting the pane")

                config = root / "config/cmux/cmux.json"
                config.parent.mkdir(parents=True, exist_ok=True)
                config.write_text(json.dumps({"shortcuts": {"bindings": {"splitRight": "ctrl+shift+k"}}}))
                app.cli("reload-config")

                app.wait_for(lambda: bool(active_text().strip()), "prompt of the new pane")
                press("ctrl+shift+k")
                app.wait_for(lambda: panes() == 3, "rebound Ctrl+Shift+K splitting the pane")

                # An unbound input reaches the terminal instead of being consumed by the application.
                app.wait_for(lambda: bool(active_text().strip()), "prompt of the third pane")
                subprocess.check_call(
                    ["xdotool", "windowfocus", "--sync", windows[-1], "type", "--clearmodifiers", "--delay", "1", "Q7Z"],
                    timeout=10,
                )
                try:
                    app.wait_for(lambda: "Q7Z" in active_text(), "typed text reaching the terminal")
                except AssertionError as error:
                    # Say where the text went: another pane means keyboard focus and the active surface differ.
                    where = {
                        row["uuid"][:8]: {"active": row["active"],
                                          "has_text": "Q7Z" in json.loads(app.cli("read-text", "--id", row["uuid"], "--json"))["text"]}
                        for row in app.surfaces()
                    }
                    raise AssertionError(f"typed text did not reach the active terminal; per surface: {where}") from error

                press("ctrl+d")
                time.sleep(1)  # no event to wait for: give a wrongly still-bound key time to act
                assert panes() == 3, "the old key still split the pane after the rebinding"
        finally:
            wm.terminate()
            wm.wait(timeout=10)
    print("rebound shortcut worked after live reload, the old key went inert, typed text reached the terminal")


if __name__ == "__main__":
    main()
