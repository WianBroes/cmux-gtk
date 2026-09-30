#!/usr/bin/env python3
"""Real-application scenarios for the CLI verbs agents use continuously (features F12, F19, F20, F21, F23).

One disposable instance; every step prints the raw output it judged, so a failure in CI shows the real format.
"""
import json
from pathlib import Path
import subprocess
import tempfile

from linux_app import running_app
from test_multi_workspace_focus import selected_surface


def main():
    """send/read-screen, new-split --command without focus theft, tree, log, and the terminal title."""
    with tempfile.TemporaryDirectory(prefix="cmux-agent-cli-") as directory:
        root = Path(directory)
        wm = subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            with running_app(root) as app:
                app.wait_for(lambda: len(app.children()) == 1, "initial shell")
                first = selected_surface(app)

                def screen(surface):
                    """Viewport text through read-screen."""
                    return app.cli("read-screen", "--surface", surface)

                app.wait_for(lambda: bool(screen(first).strip()), "first prompt")

                # F21: send expands \n, read-screen shows the executed output on its own line.
                app.cli("send", "--surface", first, "echo CMUX_SEND_OK\\n")
                try:
                    app.wait_for(lambda: "CMUX_SEND_OK" in screen(first).splitlines(), "send then read-screen")
                except AssertionError as error:
                    raw = screen(first)
                    text = json.loads(app.cli("read-text", "--id", first, "--json"))["text"]
                    raise AssertionError(f"send/read-screen: read-screen={raw!r} read-text={text!r}") from error
                print("F21 read-screen lines:", [l for l in screen(first).splitlines() if "CMUX" in l], flush=True)

                # F12: a terminal title (OSC 0) names the workspace that has no explicit name.
                # The shell restores its own title at the next prompt, so keep it busy while the name is read.
                app.cli("send", "--surface", first, "printf '\\033]0;CMUX_TITLE_X\\007'; sleep 30\\n")
                names = lambda: [row["name"] for row in json.loads(app.cli("list-workspaces", "--json"))["workspaces"]]
                app.wait_for(lambda: "CMUX_TITLE_X" in names(), "workspace named after the terminal title")
                print("F12 workspace names:", names(), flush=True)

                # F19: new-split --command keeps focus on the caller and runs the command in the new pane.
                before = {row["uuid"] for row in app.surfaces()}
                app.cli("new-split", "right", "--surface", first, "--command", "echo NEWPANE_OK")
                app.wait_for(lambda: len(app.surfaces()) == len(before) + 1, "the new pane")
                created = next(row["uuid"] for row in app.surfaces() if row["uuid"] not in before)
                assert selected_surface(app) == first, "new-split --command stole the focus"
                app.wait_for(lambda: "NEWPANE_OK" in screen(created).splitlines(), "command output in the new pane")
                print("F19 focus stayed on", first[:8], "new pane", created[:8], flush=True)

                # F20: tree lists every terminal.
                tree = app.cli("tree", "--all")
                print("F20 tree output:\n" + tree, flush=True)
                assert tree.count("surface:") >= len(app.surfaces()), "tree misses a terminal"

                # F23: the workspace log keeps entries in order.
                app.cli("log", "--level", "warning", "--source", "agent-x", "CMUX_LOG_ONE")
                app.cli("log", "CMUX_LOG_TWO")
                listing = app.cli("list-log")
                print("F23 list-log output:\n" + listing, flush=True)
                assert listing.index("CMUX_LOG_ONE") < listing.index("CMUX_LOG_TWO"), "log entries missing or out of order"
                limited = app.cli("list-log", "--limit", "1")
                assert "CMUX_LOG_TWO" in limited and "CMUX_LOG_ONE" not in limited, "list-log --limit did not keep the last entry"
        finally:
            wm.terminate()
            wm.wait(timeout=10)
    print("agent CLI scenarios verified: send/read-screen, title naming, new-split --command, tree, log")


if __name__ == "__main__":
    main()
