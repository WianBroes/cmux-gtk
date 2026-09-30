#!/usr/bin/env python3
"""Behavioural scenarios for `cmux config` and `cmux settings path` against the real binary and real files (issue 5).

None of these verbs needs the app, so the test runs the production `cmux` with an isolated XDG_CONFIG_HOME and
judges what lands on disk and in the exit code. Every check runs and prints the raw value it judged.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile

BINARY = Path(os.environ.get("CMUX_BIN_DIR", "target/debug")) / "cmux"


def main():
    """Run every scenario, print each outcome and fail at the end if any judged value was wrong."""
    results = []
    with tempfile.TemporaryDirectory(prefix="cmux-config-cli-") as directory:
        root = Path(directory)
        environment = {**os.environ, "XDG_CONFIG_HOME": str(root / "xdg"), "HOME": str(root / "home")}
        settings = root / "xdg/cmux/cmux.json"

        def cmux(*arguments):
            """Run one production CLI invocation and return (exit code, stdout, stderr)."""
            done = subprocess.run([str(BINARY), *arguments], env=environment, capture_output=True, text=True, timeout=30)
            print("  cmux", *arguments, "->", done.returncode, repr(done.stdout[:160]), repr(done.stderr[:160]), flush=True)
            return done.returncode, done.stdout, done.stderr

        def check(name, condition, seen):
            """Record one judged outcome with the value that decided it."""
            results.append((name, bool(condition), seen))
            print(("PASS " if condition else "FAIL ") + name, "|", seen, flush=True)

        code, out, _ = cmux("config", "path")
        check("config path prints the isolated global file", code == 0 and out.strip() == str(settings), out.strip())
        code, out, _ = cmux("settings", "path")
        check("settings path agrees with config path", code == 0 and out.strip() == str(settings), out.strip())

        # set keeps comments and other keys, get reads the value back.
        settings.parent.mkdir(parents=True)
        settings.write_text('{\n  // keep this comment\n  "app": {"language": "fr"}\n}\n')
        code, out, _ = cmux("config", "set", "app.appearance", "dark")
        text = settings.read_text()
        check("config set writes the value and keeps comment and neighbour key",
              code == 0 and '"dark"' in text and "// keep this comment" in text and '"language"' in text, text)
        check("config set reports persisted", '"persisted"' in out, out.strip())
        code, out, _ = cmux("config", "get", "app.appearance")
        check("config get reads the value back", code == 0 and "dark" in out, out.strip())

        # unset removes it once, then reports unchanged.
        code, out, _ = cmux("config", "unset", "app.appearance")
        text = settings.read_text()
        check("config unset removes the value and keeps the rest",
              code == 0 and "dark" not in text and "// keep this comment" in text and '"language"' in text, text)
        check("config unset reports persisted", '"persisted"' in out, out.strip())
        _, out, _ = cmux("config", "unset", "app.appearance")
        check("config unset on an absent value reports unchanged", '"unchanged"' in out, out.strip())

        # validate: exit 0 for a good file, exit 1 for an enum violation and for broken JSON.
        code, _, _ = cmux("config", "validate")
        check("config validate accepts a valid file (exit 0)", code == 0, code)
        bad = root / "bad.json"
        bad.write_text('{"app": {"appearance": "sepia"}}')
        code, out, err = cmux("config", "validate", "--file", str(bad))
        check("config validate rejects a value outside the enum (exit 1)", code == 1, f"exit {code}")
        broken = root / "broken.json"
        broken.write_text("{broken")
        code, _, _ = cmux("config", "validate", "--file", str(broken))
        check("config validate rejects broken JSON (non-zero exit)", code != 0, f"exit {code}")

        # set --file with an explicit project file writes that file, not the global one.
        project = root / "project/cmux.json"
        project.parent.mkdir()
        before = settings.read_text()
        cmux("config", "set", "app.appearance", "light", "--file", str(project))
        check("config set --file writes the named file only",
              project.exists() and '"light"' in project.read_text() and settings.read_text() == before,
              project.read_text() if project.exists() else "missing")

        code, out, _ = cmux("config", "list-supported")
        check("config list-supported lists the schema paths", code == 0 and "app.appearance" in out, out[:120])
        code, out, _ = cmux("config", "docs")
        check("config docs prints the reference", code == 0 and "cmux.json" in out, out[:120])

    failed = [name for name, ok, _ in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} config checks passed", flush=True)
    if failed:
        raise AssertionError("config behaviour wrong: " + "; ".join(failed))
    print("cmux config verbs behave against the real binary and files")


if __name__ == "__main__":
    main()
