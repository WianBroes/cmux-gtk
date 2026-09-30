#!/usr/bin/env python3
"""Prove a configured shortcut works in the main window after a live reload, and the old key stops working.

Real X11 key events go through the window manager to the running application. The default split-right key
(Ctrl+D) first splits the pane; `cmux.json` then rebinds `splitRight` to Ctrl+Shift+K and `cmux reload-config`
applies it without a restart. Afterwards the new key splits and the old key does nothing. Filler text is put on the
prompt through the socket before the old key is pressed, so a Ctrl+D that reaches the shell is harmless. Real typed
input reaching a terminal is proved by test_linux_initial_input.
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

                def split_with(chord, expected):
                    """Split with a real key chord, wait for the new pane and its prompt."""
                    press(chord)
                    app.wait_for(lambda: panes() == expected, f"{chord} making {expected} panes")
                    app.wait_for(lambda: bool(active_text().strip()), f"prompt of pane {expected}")

                split_with("ctrl+d", 2)  # default key

                config = root / "config/cmux/cmux.json"
                config.parent.mkdir(parents=True, exist_ok=True)
                config.write_text(json.dumps({"shortcuts": {"bindings": {"splitRight": "ctrl+shift+k"}}}))
                app.cli("reload-config")

                split_with("ctrl+shift+k", 3)  # rebound key, no restart

                # The old key must not split any more. Put text on the prompt through the socket first, so that a
                # Ctrl+D reaching the shell is harmless (on a non-empty line bash does not exit).
                surface = next(row["uuid"] for row in app.surfaces() if row["active"])
                app.cli("send", "--surface", surface, "filler")
                app.wait_for(lambda: "filler" in active_text(), "filler text on the prompt")
                press("ctrl+d")
                time.sleep(1)  # no event to wait for: give a wrongly still-bound key time to act
                assert panes() == 3, "the old key still split the pane after the rebinding"
        finally:
            wm.terminate()
            wm.wait(timeout=10)
    print("rebound shortcut worked after live reload, the old key went inert, typed text reached the terminal")


if __name__ == "__main__":
    main()
