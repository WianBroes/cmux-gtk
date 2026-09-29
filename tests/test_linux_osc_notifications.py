#!/usr/bin/env python3
"""Exercise OSC notifications through actual PTY output from an inactive sibling terminal tab."""
import json
from pathlib import Path
import shlex
import tempfile

from linux_app import running_app
from test_linux_resume_approval import key, window

PROBE = r'''
import base64, os, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
def wait(name):
    deadline = time.monotonic() + 15
    while not (root / name).exists():
        if time.monotonic() > deadline:
            raise RuntimeError(name)
        time.sleep(.01)
(root / 'ready').touch()
wait('start')
os.write(1, b'\x1b]9;first burst message\x07')
(root / 'osc9').touch()
wait('go777')
os.write(1, b'\x1b]777;notify;Second title;second burst body\x1b\\')
os.write(1, b'\x1b]99;i=chunk:d=0;Chunk title\x1b\\')
(root / 'partial').touch()
wait('finish')
payload = b'\x1b]99;i=chunk:p=body:e=1;' + base64.b64encode(b'long body ' * 100) + b'\x1b\\'
for byte in payload:
    os.write(1, bytes([byte]))
(root / 'chunked').touch()
wait('oversize')
os.write(1, b'\x1b]9;' + b'x' * 20000 + b'\x07\x1b]9;after oversized frame\x07')
(root / 'finished').touch()
'''


def main():
    """Preserve full content, burst delivery and chunk boundaries without selecting the source tab."""
    with tempfile.TemporaryDirectory(prefix="cmux-osc-") as directory:
        root = Path(directory)
        script = root / "probe.py"
        script.write_text(PROBE)
        with running_app(root) as app:
            app.wait_for(lambda: len(app.children()) == 1, "initial native shell")
            source = next(row["uuid"] for row in app.surfaces() if row["active"])
            app.cli("send-text", "--id", source, "python3 " + shlex.quote(str(script)) + " " + shlex.quote(str(root)))
            app.cli("send-key", "--id", source, "\r")
            app.wait_for((root / "ready").exists, "owned PTY probe")
            key(window(app), "ctrl+t")
            app.wait_for(lambda: len(app.surfaces()) == 2, "sibling terminal")
            selected = next(row["uuid"] for row in app.surfaces() if row["active"])
            assert selected != source

            def messages():
                """Read notifications without forcing native rendering or changing focus."""
                return json.loads(app.cli("notifications", "list", "--json"))["notifications"]

            def only(description, predicate):
                """Wait for the single notification of the source terminal (one per terminal, as on macOS)."""
                app.wait_for(lambda: len(messages()) == 1 and predicate(messages()[0]), description)
                assert messages()[0]["surface_id"] == source and not messages()[0]["is_read"]

            (root / "start").touch()
            app.wait_for((root / "osc9").exists, "OSC9 output")
            only("OSC9 delivery", lambda row: row["body"] == "first burst message")
            (root / "go777").touch()
            app.wait_for((root / "partial").exists, "incomplete chunk output")
            only("OSC777 replacing OSC9 while the OSC99 chunk is incomplete",
                 lambda row: row["title"] == "Second title" and row["body"] == "second burst body")
            (root / "finish").touch()
            app.wait_for((root / "chunked").exists, "fragmented PTY output")
            only("completed OSC99 chunk", lambda row: row["title"] == "Chunk title" and row["body"] == "long body " * 100)
            (root / "oversize").touch()
            app.wait_for((root / "finished").exists, "oversized frame output")
            only("parser recovery after an oversized frame", lambda row: row["body"] == "after oversized frame")
            assert next(row["uuid"] for row in app.surfaces() if row["active"]) == selected
            metrics = json.loads(app.cli("diagnostics", "--json"))["notification_parser"]
            assert metrics["accepted"] >= 4 and metrics["oversize_frames"] >= 1
            assert metrics["output_bytes"] > 20000 and metrics["parse_ns"] > 0
    print("PTY OSC9/777 bursts and fragmented OSC99 retained full content and exact sibling identity")


if __name__ == "__main__":
    main()
