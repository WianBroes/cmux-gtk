#!/usr/bin/env python3
"""Behavioural scenarios for the browser verbs geolocation, offline and network route/unroute (issues 2 and 3).

A real page is served over local http and driven through a real daemon and Chromium. Every check runs even
when an earlier one fails, and each prints the raw value it judged, so one CI run shows the state of all verbs.
Needs agent-browser >= 0.38.1 (older daemons do not intercept routes); the version used is printed.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from linux_app import running_app

PAGE = b'<!doctype html><title>verbs</title><h1 id=x>verbs page</h1><a id=dl href="/file.txt" download>get</a>'
DATA = b'{"who": "real-server"}'


class Handler(BaseHTTPRequestHandler):
    """Serve the page and one JSON endpoint, uncached so an offline or route change is never masked."""

    def do_GET(self):
        if self.path.startswith("/file"):
            body, kind = b"download-body", "text/plain"
        elif self.path.startswith("/page"):
            body, kind = PAGE, "text/html"
        else:
            body, kind = DATA, "application/json"
        self.send_response(200)
        if self.path.startswith("/file"):
            self.send_header("Content-Disposition", 'attachment; filename="file.txt"')
        self.send_header("Content-Type", kind)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        """Keep the CI log free of request lines."""


def main():
    """Run every verb scenario, print each raw outcome, and fail at the end if any judged value was wrong."""
    browser = os.environ.get("CMUX_TEST_AGENT_BROWSER") or shutil.which("agent-browser")
    if browser is None:
        raise RuntimeError("agent-browser >= 0.38.1 is required")
    version = subprocess.run([browser, "--version"], capture_output=True, text=True, timeout=20).stdout.strip()
    print("agent-browser version:", version, flush=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    results = []
    with tempfile.TemporaryDirectory(prefix="cmux-real-verbs-") as directory:
        root = Path(directory)
        browser_dir = root / "browser"
        browser_dir.mkdir()
        with running_app(root, {"CMUX_AGENT_BROWSER": browser, "AGENT_BROWSER_SOCKET_DIR": str(browser_dir)}) as app:
            app.wait_for(lambda: bool(app.children()), "initial terminal")
            opened = json.loads(app.cli("browser", "open", base + "/page.html", timeout=40))
            print("browser open ->", opened, flush=True)
            surface = opened.get("surface_ref") or opened.get("data", {}).get("surface_ref") or opened.get("surface_id")

            def verb(*arguments):
                """Run one `cmux browser` verb and return its parsed JSON answer."""
                answer = json.loads(app.cli("browser", *arguments, timeout=30))
                print("  cmux browser", *arguments, "->", json.dumps(answer)[:300], flush=True)
                return answer

            def page(expression):
                """Evaluate an async expression in the page and return its string result."""
                answer = verb("eval", surface, expression)
                return answer.get("data", {}).get("result")

            fetch_data = "fetch('/data.json',{cache:'no-store'}).then(r=>r.text()).catch(e=>'FETCH-ERROR')"
            locate = ("new Promise(r=>navigator.geolocation.getCurrentPosition("
                      "p=>r('ok:'+p.coords.latitude.toFixed(2)+','+p.coords.longitude.toFixed(2)),"
                      "e=>r('err:'+e.message),{timeout:5000}))")

            def check(name, condition, seen):
                """Record one judged outcome with the value that decided it."""
                results.append((name, bool(condition), seen))
                print(("PASS " if condition else "FAIL ") + name, "|", seen, flush=True)

            # Baseline: the real endpoint answers.
            seen = page(fetch_data)
            check("baseline fetch reaches the real server", seen and "real-server" in seen, seen)

            # Issue 3: the page must be able to read the emulated position.
            verb("geolocation", surface, "48.85", "2.35")
            seen = page(locate)
            check("geolocation: page reads the emulated position", seen == "ok:48.85,2.35", seen)

            # Issue 2: offline blocks fetch, going back online restores it.
            verb("offline", surface, "on")
            seen = page(fetch_data)
            check("offline on: fetch fails", seen == "FETCH-ERROR", seen)
            verb("offline", surface, "off")
            seen = page(fetch_data)
            check("offline off: fetch works again", seen and "real-server" in seen, seen)

            # Route abort, mocked body, then unroute restores the real answer.
            verb("network", surface, "route", base + "/data.json", "--abort")
            seen = page(fetch_data)
            check("route --abort: fetch fails", seen == "FETCH-ERROR", seen)
            verb("network", surface, "unroute", base + "/data.json")
            seen = page(fetch_data)
            check("unroute after abort: real answer returns", seen and "real-server" in seen, seen)
            verb("network", surface, "route", base + "/data.json", "--body", '{"who":"mocked"}')
            seen = page(fetch_data)
            check("route --body: page receives the mocked body", seen and "mocked" in seen, seen)
            verb("network", surface, "unroute", base + "/data.json")
            seen = page(fetch_data)
            check("unroute after body: real answer returns", seen and "real-server" in seen, seen)

            # Trace and HAR recorders write real files with the traffic seen while recording.
            trace_file = root / "recorded-trace.json"
            verb("trace", surface, "start")
            page(fetch_data)
            verb("trace", surface, "stop", str(trace_file))
            size = trace_file.stat().st_size if trace_file.exists() else 0
            check("trace start/stop: a non-empty trace file is written", size > 0, f"{trace_file.name} {size} bytes")
            har_file = root / "recorded.har"
            verb("har", surface, "start")
            page(fetch_data)
            verb("har", surface, "stop", str(har_file))
            try:
                urls = [entry["request"]["url"] for entry in json.loads(har_file.read_text())["log"]["entries"]]
            except (OSError, ValueError, KeyError) as error:
                urls = [f"unreadable HAR: {error!r}"]
            check("har start/stop: valid HAR containing the request made", any(url.endswith("/data.json") for url in urls), urls[:5])

            # Issue 5: emulation, cookies, storage, scripts, styles and downloads must change the real page.
            verb("viewport", surface, "800", "600")
            seen = page("innerWidth+'x'+innerHeight")
            check("viewport: page sees the requested size", seen == "800x600", seen)

            verb("cookies", surface, "set", "cmuxk", "cmuxv", "--url", base)
            seen = page("document.cookie")
            check("cookies set: page sees the cookie", seen and "cmuxk=cmuxv" in seen, seen)
            got = verb("cookies", surface, "get")
            check("cookies get: the cookie is listed", "cmuxk" in json.dumps(got), json.dumps(got)[:200])
            verb("cookies", surface, "clear")
            seen = page("document.cookie")
            check("cookies clear: page has no cookie left", seen == "", seen)

            page("localStorage.setItem('storek','storev'); 'set'")
            got = verb("storage", surface, "local")
            check("storage local: the stored key is returned", "storek" in json.dumps(got) and "storev" in json.dumps(got), json.dumps(got)[:200])

            verb("addstyle", surface, "#x{color:rgb(1,2,3)}")
            seen = page("getComputedStyle(document.getElementById('x')).color")
            check("addstyle: the page style changes", seen == "rgb(1, 2, 3)", seen)

            verb("addscript", surface, "window.__added = 42")
            seen = page("String(window.__added)")
            check("addscript: the script ran in the page", seen == "42", seen)

            verb("addinitscript", surface, "window.__init = 'yes'")
            page("setTimeout(()=>location.reload(),50); 'reloading'")
            time.sleep(2)
            seen = page("String(window.__init)")
            check("addinitscript: runs before page scripts after a reload", seen == "yes", seen)

            download_file = root / "downloaded.txt"
            verb("download", surface, "#dl", str(download_file))
            content = download_file.read_text() if download_file.exists() else None
            check("download: the clicked link is saved to the file", content == "download-body", content)
    server.shutdown()
    failed = [name for name, ok, _ in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} verb checks passed", flush=True)
    if failed:
        raise AssertionError("verbs that did not behave: " + "; ".join(failed))
    print("browser verbs behave against a real daemon and Chromium")


if __name__ == "__main__":
    main()
