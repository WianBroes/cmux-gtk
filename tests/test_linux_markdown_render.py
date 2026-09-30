#!/usr/bin/env python3
"""Judge the page `cmux markdown` writes, without an application or a browser.

`cmux markdown <file>` renders the file to a self-contained page under the data directory before it contacts the
socket, so the real binary writes the page even when no application is running. This test pins that page: it runs
the production CLI with isolated XDG roots and an explicitly absent socket, expects the command to fail on the
missing socket, then judges the HTML that was written. Every check prints the raw value it judged.

The page itself is proved to reach a real browser by `tests/test_linux_real_markdown_view.py`.
"""
import os
from pathlib import Path
import subprocess
import tempfile

BINARY = Path(os.environ.get("CMUX_BIN_DIR", "target/debug")) / "cmux"

MARKDOWN = (
    "# Deployment notes\n\n"
    "First paragraph.\n\n"
    "- first step\n"
    "- second step\n\n"
    "A [link](https://example.test/docs) and [bad](javascript:alert).\n"
)


def main():
    """Run the production CLI and judge the rendered page it wrote to disk."""
    results = []
    with tempfile.TemporaryDirectory(prefix="cmux-markdown-render-") as directory:
        root = Path(directory)
        (root / "runtime").mkdir()
        (root / "home").mkdir()
        document = root / "notes.md"
        document.write_text(MARKDOWN)
        environment = {
            **os.environ,
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_RUNTIME_DIR": str(root / "runtime"),
            "HOME": str(root / "home"),
            "CMUX_SOCKET": str(root / "runtime/cmux/cmux.sock"),
        }
        done = subprocess.run([str(BINARY), "markdown", str(document)], env=environment,
                              capture_output=True, text=True, timeout=30)
        print("cmux markdown ->", done.returncode, repr(done.stdout[:200]), repr(done.stderr[:200]), flush=True)

        def check(name, condition, seen):
            """Record one judged outcome with the value that decided it."""
            results.append((name, bool(condition), seen))
            print(("PASS " if condition else "FAIL ") + name, "|", seen, flush=True)

        def around(needle):
            """Show the judged markup without hiding it behind a missing marker."""
            start = html.find(needle)
            return "absent" if start < 0 else html[start:start + 60]

        written = sorted((root / "data/cmux/diffs").glob("markdown-*.html")) if (root / "data/cmux/diffs").is_dir() else []
        check("the page was written next to the diff viewers", len(written) == 1, [str(path) for path in written])
        html = written[0].read_text() if len(written) == 1 else ""
        check("the file's heading is an h1", "<h1>Deployment notes</h1>" in html, around("<h1>"))
        check("both list items are list items", "<li>first step</li>" in html and "<li>second step</li>" in html,
              around("<ul>"))
        check("the paragraph text is rendered", "<p>First paragraph.</p>" in html, around("First paragraph"))
        check("only the http link becomes a clickable link",
              html.count("<a ") == 1 and '<a href="https://example.test/docs">' in html,
              f"{html.count('<a ')} anchor(s), {around('href=')}")
        check("the page title is the file's heading", "<title>Deployment notes</title>" in html, around("<title>"))
        check("the page carries no script tag", "<script" not in html, "script present" if "<script" in html else "absent")
        check("the command refuses to open a surface without the application", done.returncode != 0, done.returncode)

    failed = [name for name, ok, _ in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} markdown render checks passed", flush=True)
    if failed:
        raise AssertionError("markdown render wrong: " + "; ".join(failed))
    print("cmux markdown wrote the expected page for a real file")


if __name__ == "__main__":
    main()
