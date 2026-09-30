#!/usr/bin/env python3
"""Break one line of the product on purpose and require its scenario test to fail.

Usage: run_mutation.py <case id>. Exit 0 means the test caught the change (mutation killed);
1 means the test still passed (mutation survived, the test proves nothing about the claim);
2 means the mutation itself is invalid (stale search text or the broken code does not compile).
"""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def main():
    """Apply the named mutation, rebuild, run the test under Xvfb and report kill or survival."""
    case = next(row for row in json.loads((ROOT / "tests/mutation_cases.json").read_text()) if row["id"] == sys.argv[1])
    path = ROOT / case["file"]
    source = path.read_text()
    if source.count(case["old"]) != 1:
        print(f"INVALID: search text found {source.count(case['old'])} times in {case['file']}")
        return 2
    path.write_text(source.replace(case["old"], case["new"]))
    print(f"claim: {case['claim']}\nmutation: {case['file']}: {case['old'].strip()!r} -> {case['new'].strip()!r}", flush=True)
    build = subprocess.run(["cargo", "build", "--workspace", "--bins"], cwd=ROOT)
    if build.returncode:
        print("INVALID: mutated code does not compile")
        return 2
    result = subprocess.run(["dbus-run-session", "--", "xvfb-run", "-a", "python3", case["test"]], cwd=ROOT, timeout=600)
    if result.returncode:
        print(f"KILLED: {case['test']} failed with the mutation (exit {result.returncode})")
        return 0
    print(f"SURVIVED: {case['test']} still passes with the mutation")
    return 1


if __name__ == "__main__":
    sys.exit(main())
