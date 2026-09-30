#!/usr/bin/env python3
"""Render the generated self-contained Markdown preview in pinned Chromium.

`cmux markdown <file>` renders the file next to the diff viewer and opens it in a browser surface right of the
calling terminal. A real agent-browser drives the page, so the checks judge what the page really shows. Every
check runs even when an earlier one fails and prints the raw value it judged, so one CI run shows the whole state.
"""
import json
import os
from pathlib import Path
import shutil
import tempfile

from browser_process_support import BrowserProcesses
from linux_app import running_app
from test_multi_workspace_focus import selected_surface

MARKDOWN = (
    "# Deployment notes\n\n"
    "First paragraph.\n\n"
    "- first step\n"
    "- second step\n\n"
    "A [link](https://example.test/docs) and [bad](javascript:alert).\n"
)


def main():
    """Open a real Markdown file through the production CLI and judge the rendered page."""
    browser = os.environ.get("CMUX_TEST_AGENT_BROWSER") or shutil.which("agent-browser")
    if browser is None:
        raise RuntimeError("agent-browser is required for the real markdown fixture")
    results = []
    with tempfile.TemporaryDirectory(prefix="cmux-real-markdown-") as directory:
        root = Path(directory)
        browser_dir = root / "browser"
        browser_dir.mkdir()
        document = root / "notes.md"
        document.write_text(MARKDOWN)
        processes = BrowserProcesses(browser_dir)
        with running_app(root, {
            "CMUX_AGENT_BROWSER": browser,
            "AGENT_BROWSER_SOCKET_DIR": str(browser_dir),
        }) as app:
            app.wait_for(lambda: bool(app.children()), "initial terminal")
            terminal = selected_surface(app)
            opened = json.loads(app.cli("markdown", str(document), "--json", timeout=35))
            print("markdown open ->", json.dumps(opened)[:300], flush=True)
            surface = opened["surface_ref"]

            def command(name, *arguments):
                """Run one browser verb against the preview surface and return its raw data."""
                answer = json.loads(app.cli("browser", name, surface, *arguments, timeout=20))
                print("  cmux browser", name, *arguments, "->", json.dumps(answer)[:300], flush=True)
                assert answer["success"] is True, answer
                return answer["data"]

            def check(name, condition, seen):
                """Record one judged outcome with the value that decided it."""
                results.append((name, bool(condition), seen))
                print(("PASS " if condition else "FAIL ") + name, "|", seen, flush=True)

            command("wait", "--selector", "h1", "--timeout-ms", "5000")
            state = command("eval", (
                "({title:document.title,"
                "heading:document.querySelector('h1')?document.querySelector('h1').textContent:null,"
                "items:[...document.querySelectorAll('li')].map(x=>x.textContent),"
                "paragraphs:[...document.querySelectorAll('p')].map(x=>x.textContent),"
                "hrefs:[...document.querySelectorAll('a')].map(x=>x.getAttribute('href')),"
                "scripts:document.querySelectorAll('script').length})"
            ))["result"]
            check("the file's heading renders as an h1", state["heading"] == "Deployment notes", state["heading"])
            check("the list renders as list items", state["items"] == ["first step", "second step"], state["items"])
            check("the paragraph text is rendered",
                  state["paragraphs"] == ["First paragraph.", "A link and bad."], state["paragraphs"])
            check("only the http link becomes an anchor", state["hrefs"] == ["https://example.test/docs"], state["hrefs"])
            check("the page carries no script tag", state["scripts"] == 0, state["scripts"])
            check("the page title is the file's heading", state["title"] == "Deployment notes", state["title"])
            check("the preview is a surface of the running app",
                  any(row["uuid"] == surface for row in app.surfaces()),
                  [row["uuid"] for row in app.surfaces()])
            check("the terminal keeps the focus", selected_surface(app) == terminal, selected_surface(app))
            check("one browser daemon serves the preview", processes.sample()["daemon_count"] == 1,
                  processes.sample()["daemon_count"])

    failed = [name for name, ok, _ in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} markdown checks passed", flush=True)
    if failed:
        raise AssertionError("markdown preview wrong: " + "; ".join(failed))
    print("real Chromium rendered the bounded Markdown preview through browser automation")


if __name__ == "__main__":
    main()
