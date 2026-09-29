#!/usr/bin/env python3
"""Prove the agent terminal I/O verbs (send-text, send-key, read-text) against a real shell.

The shell must execute what was sent and the output must come back through read-text; a
background surface must receive input without stealing focus.
"""
import json
from pathlib import Path
import tempfile

from linux_app import running_app
from test_multi_workspace_focus import selected_surface


def main():
    """Send a command by text plus a literal Enter to the focused and to a background terminal."""
    with tempfile.TemporaryDirectory(prefix="cmux-agent-io-") as directory:
        with running_app(Path(directory)) as app:
            app.wait_for(lambda: len(app.children()) == 1, "initial shell")
            first = selected_surface(app)
            app.cli("new-workspace")
            app.wait_for(lambda: len(app.children()) == 2, "second shell")
            focused = selected_surface(app)
            assert focused != first

            def text(surface):
                """Read the viewport of one surface without changing focus."""
                return json.loads(app.cli("read-text", "--id", surface, "--json"))["text"]

            app.wait_for(lambda: bool(text(first).strip()) and bool(text(focused).strip()), "prompts")
            for surface, marker in ((focused, "CMUX_IO_FOCUSED"), (first, "CMUX_IO_BACKGROUND")):
                app.cli("send-text", f"printf '{marker}_%s\\n' 7", "--id", surface)
                app.wait_for(lambda: f"printf '{marker}_%s" in text(surface), "typed but unsubmitted text")
                assert f"{marker}_7" not in text(surface).splitlines(), "executed before Enter"
                app.cli("send-key", "\r" if surface == focused else "enter", "--id", surface)  # literal character, then a named key
                app.wait_for(lambda: f"{marker}_7" in text(surface).splitlines(), "executed output")
                assert selected_surface(app) == focused, "input stole focus"
    print("send-text, send-key and read-text drove focused and background shells without focus change")


if __name__ == "__main__":
    main()
