#!/usr/bin/env python3
"""Real-application scenario for F18: socket references, target handles and `--id-format`.

One disposable instance and its production CLI. `window:N` / `workspace:N` / `surface:N` references,
the `--workspace` flag, a bare index and the identity shaping of `--id-format` are all judged on
what the CLI really answers. Every control exists even when an earlier one fails, prints the raw
value it judged, and the final failure lists the false ones.

The `CMUX_SOCKET_PATH` fallback of F18 is judged by `tests/test_cli_socket_autodiscovery.py`,
which needs no display; it is not repeated here.
"""
import json
import subprocess
import tempfile
from pathlib import Path

from linux_app import running_app
from test_multi_workspace_focus import selected_surface


def main():
    """Run every control, print each outcome and fail at the end if any judged value was wrong."""
    results = []
    with tempfile.TemporaryDirectory(prefix="cmux-handles-") as directory:
        root = Path(directory)
        wm = subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            with running_app(root) as app:
                binary = Path(app.environment.get("CMUX_BIN_DIR", "target/debug")) / "cmux"

                def cmux(*arguments, expect_success=True):
                    """Run one production CLI call; return (exit code, stdout+stderr) without raising."""
                    done = subprocess.run(
                        [str(binary), "--socket", str(app.socket_path), *arguments],
                        env=app.environment, capture_output=True, text=True, timeout=20,
                    )
                    output = (done.stdout + done.stderr).strip()
                    print("  cmux", *arguments, "->", done.returncode, repr(output[:200]), flush=True)
                    if expect_success and done.returncode != 0:
                        raise AssertionError(f"cmux {' '.join(arguments)} failed: {done.returncode} {output!r}")
                    return done.returncode, output

                def check(name, condition, seen):
                    """Record one judged outcome with the value that decided it."""
                    results.append((name, bool(condition), seen))
                    print(("PASS " if condition else "FAIL ") + name, "|", seen, flush=True)

                def workspaces(*arguments):
                    """Workspace records as the real CLI prints them."""
                    return json.loads(app.cli("list-workspaces", "--json", *arguments))["workspaces"]

                app.wait_for(lambda: len(app.children()) == 1, "initial shell")
                app.wait_for(lambda: bool(app.cli("read-screen").strip()), "first prompt")
                first = selected_surface(app)

                # References and UUIDs are reported side by side by the server.
                surfaces = app.surfaces()
                check("the first terminal is surface:1 beside its UUID",
                      surfaces[0]["ref"] == "surface:1" and surfaces[0]["id"] == surfaces[0]["uuid"] == first,
                      json.dumps(surfaces[0], sort_keys=True))

                focused = json.loads(app.cli("identify", "--json"))["focused"]
                check("identify --json reports the focused window/workspace/surface refs beside their UUIDs",
                      focused["window_ref"] == "window:1" and focused["workspace_ref"] == "workspace:1"
                      and focused["surface_ref"] == "surface:1" and focused["surface_id"] == first,
                      json.dumps(focused, sort_keys=True))

                text = app.cli("identify").splitlines()
                check("identify prints the version line and the focused references",
                      text[0].startswith("cmux ") and "(linux)" in text[0]
                      and any(line.startswith("focused: ") and "window:1" in line for line in text),
                      repr(text))

                # `--id-format` drops the redundant half of each identity, in both directions.
                refs = json.loads(app.cli("list-surfaces", "--json", "--id-format", "refs"))["surfaces"]
                check("--id-format refs drops the UUID half of every identity",
                      all("id" not in row and "pane_id" not in row and "workspace_id" not in row
                          and row["ref"] and row["pane_ref"] and row["workspace_ref"] for row in refs),
                      json.dumps(refs, sort_keys=True))

                uuids = json.loads(app.cli("list-surfaces", "--json", "--id-format", "uuids"))["surfaces"]
                check("--id-format uuids drops the reference half of every identity",
                      all("ref" not in row and "pane_ref" not in row and "workspace_ref" not in row
                          and row["id"] == row["uuid"] for row in uuids),
                      json.dumps(uuids, sort_keys=True))

                # A surface:N reference reaches the terminal; a reference of another kind is refused.
                code, screen = cmux("read-screen", "--surface", "surface:1", expect_success=False)
                check("read-screen accepts a surface:N reference for the first terminal",
                      code == 0 and bool(screen.strip()), repr(screen.splitlines()[-1:]))

                code, output = cmux("read-screen", "--surface", "workspace:1", expect_success=False)
                check("a reference of another kind is refused with the documented shape",
                      code != 0 and "Invalid surface handle: workspace:1" in output
                      and "expected UUID, ref like surface:1, or index" in output, f"exit {code}")

                # The --workspace flag and a bare index both name the workspace the server lists.
                app.cli("new-workspace", "--name", "CMUX_SECOND")
                app.wait_for(lambda: len(workspaces()) == 2, "the second workspace")
                before = [row["name"] for row in workspaces()]

                cmux("rename-workspace", "--workspace", "workspace:2", "CMUX_FLAG_OK")
                flagged = workspaces()
                check("the --workspace flag resolves workspace:2 to the second workspace",
                      flagged[1]["name"] == "CMUX_FLAG_OK" and flagged[0]["name"] == before[0],
                      json.dumps([row["name"] for row in flagged]))

                target = flagged[1]
                cmux("rename-workspace", str(target["index"]), "CMUX_INDEX_OK")
                indexed = workspaces()
                check("a bare index names the workspace the server lists under that index",
                      indexed[1]["id"] == target["id"] and indexed[1]["name"] == "CMUX_INDEX_OK"
                      and indexed[0]["name"] == before[0],
                      json.dumps([f"{row['index']}:{row['name']}" for row in indexed]))

                # Giving the handle twice, once as a positional and once as its flag, changes nothing.
                names = [row["name"] for row in workspaces()]
                code, output = cmux("rename-workspace", "--workspace", "workspace:2", "0", "CMUX_CONFLICT",
                                    expect_success=False)
                check("the positional handle and its flag together are refused without renaming anything",
                      code != 0 and [row["name"] for row in workspaces()] == names, f"exit {code}")
        finally:
            wm.terminate()
            wm.wait(timeout=10)

    failed = [name for name, ok, _ in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} handle checks passed", flush=True)
    if failed:
        raise AssertionError("handle behaviour wrong: " + "; ".join(failed))
    print("socket references, target handles and --id-format behave against the real application")


if __name__ == "__main__":
    main()
