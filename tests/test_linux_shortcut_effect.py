#!/usr/bin/env python3
"""Prove a configured shortcut works in the main window after a live reload, and the old key stops working.

Real X11 key events go through the window manager to the running application: the default New workspace key
(Ctrl+N) first creates a workspace, then `cmux.json` rebinds it to Ctrl+Shift+K and `cmux reload-config` applies
that without a restart; afterwards the new key creates a workspace, the old one does nothing, and an unbound
key still reaches the terminal.
"""
import json
from pathlib import Path
import subprocess
import tempfile
import time

from linux_app import running_app


def main():
    """Default key, live rebinding, old key inert, unbound key delivered to the terminal."""
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

                def workspaces():
                    """Count workspaces through the production CLI."""
                    return len(json.loads(app.cli("list-workspaces", "--json"))["workspaces"])

                def press(chord):
                    """Deliver a real key chord to the focused main window through the window manager."""
                    subprocess.check_call(
                        ["xdotool", "windowfocus", "--sync", windows[-1], "key", "--clearmodifiers", chord], timeout=10,
                    )

                assert workspaces() == 1
                press("ctrl+n")
                app.wait_for(lambda: workspaces() == 2, "default Ctrl+N creating a workspace")

                config = root / "config/cmux/cmux.json"
                config.parent.mkdir(parents=True, exist_ok=True)
                config.write_text(json.dumps({"shortcuts": {"bindings": {"newTab": "ctrl+shift+k"}}}))
                app.cli("reload-config")

                press("ctrl+shift+k")
                app.wait_for(lambda: workspaces() == 3, "rebound Ctrl+Shift+K creating a workspace")
                press("ctrl+n")
                time.sleep(1)  # no event to wait for: give a wrongly still-bound key time to act
                assert workspaces() == 3, "the old key still created a workspace after the rebinding"

                # An unbound key is not consumed by the application: typed text reaches the terminal.
                surface = next(row["uuid"] for row in app.surfaces() if row["active"])
                subprocess.check_call(
                    ["xdotool", "windowfocus", "--sync", windows[-1], "type", "--clearmodifiers", "--delay", "1", "Q7Z"],
                    timeout=10,
                )
                app.wait_for(
                    lambda: "Q7Z" in json.loads(app.cli("read-text", "--id", surface, "--json"))["text"],
                    "unbound keys reaching the terminal",
                )
        finally:
            wm.terminate()
            wm.wait(timeout=10)
    print("configured shortcut worked after live reload, old key went inert, unbound keys reached the terminal")


if __name__ == "__main__":
    main()
