#!/usr/bin/env python3
"""Discover PCB repos on GitHub, scan them, and upload boards to pastebom.com.

Designed to run hourly via cron. Rotates through search queries,
processes up to MAX_REPOS_PER_RUN new repos per invocation.
"""

import json
import os
import subprocess
import sys
import time

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
SCAN_SCRIPT = os.path.join(SCRIPT_DIR, "scan_github_repo.py")
RESULTS_FILE = os.path.join(SCRIPT_DIR, "scan_results.json")
STATE_FILE = os.path.join(SCRIPT_DIR, "discover_state.json")

MAX_REPOS_PER_RUN = 5
SEARCH_LIMIT = 30
UPLOAD_DELAY_SECS = 2

SEARCH_QUERIES = [
    "kicad pcb",
    "eagle pcb",
    "altium pcb",
    "gerber pcb",
    "gdsii",
    "odb++ pcb",
]


def load_state() -> dict:
    if os.path.exists(STATE_FILE):
        with open(STATE_FILE) as f:
            return json.load(f)
    return {"query_index": 0, "skip_repos": []}


def save_state(state: dict) -> None:
    with open(STATE_FILE, "w") as f:
        json.dump(state, f, indent=2)


def load_results() -> dict:
    if os.path.exists(RESULTS_FILE):
        with open(RESULTS_FILE) as f:
            return json.load(f)
    return {"scanned": {}}


def discover_repos(query: str) -> list[str]:
    """Run a gh search command and extract owner/repo names."""
    cmd = [
        "gh", "search", "repos",
        "--limit", str(SEARCH_LIMIT),
        "--sort", "updated",
        "--json", "fullName",
        "--", query,
    ]
    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        print(f"  Search failed: {result.stderr.strip()}", file=sys.stderr)
        return []

    try:
        entries = json.loads(result.stdout)
    except json.JSONDecodeError:
        print(f"  Failed to parse search results", file=sys.stderr)
        return []

    return [e["fullName"] for e in entries if "fullName" in e]


def should_skip(repo: str, results: dict, skip_list: list[str]) -> str | None:
    """Return a reason string if repo should be skipped, else None."""
    if repo in skip_list:
        return "in skip list"

    if repo in results.get("scanned", {}):
        entry = results["scanned"][repo]
        matches = entry.get("matches", [])
        if not matches:
            return "no PCB files"
        all_uploaded = all(m.get("upload_url") for m in matches)
        if all_uploaded:
            return "all uploaded"

    return None


def check_repo_quality(repo: str) -> str | None:
    """Check if repo is worth processing. Returns skip reason or None."""
    result = subprocess.run(
        ["gh", "repo", "view", repo, "--json", "isArchived,isFork,stargazerCount"],
        capture_output=True,
        text=True,
        timeout=15,
    )
    if result.returncode != 0:
        return "could not fetch repo info"

    try:
        info = json.loads(result.stdout)
    except json.JSONDecodeError:
        return "could not parse repo info"

    if info.get("isFork"):
        return "fork"

    return None


def run_scan(repo: str) -> bool:
    """Run scan_github_repo.py scan. Returns True if successful."""
    result = subprocess.run(
        [sys.executable, SCAN_SCRIPT, "scan", repo, "--results", RESULTS_FILE],
        timeout=120,
    )
    return result.returncode == 0


def run_upload(repo: str) -> bool:
    """Run scan_github_repo.py upload. Returns True if successful."""
    result = subprocess.run(
        [sys.executable, SCAN_SCRIPT, "upload", repo, "--results", RESULTS_FILE],
        timeout=300,
    )
    return result.returncode == 0


def main():
    state = load_state()
    results = load_results()

    query_index = state["query_index"] % len(SEARCH_QUERIES)
    query = SEARCH_QUERIES[query_index]
    state["query_index"] = (query_index + 1) % len(SEARCH_QUERIES)

    print(f"=== Discovery run (query {query_index + 1}/{len(SEARCH_QUERIES)}) ===")
    print(f"Query: {query}")
    print()

    repos = discover_repos(query)
    print(f"Found {len(repos)} repo(s) from search")

    processed = 0
    for repo in repos:
        if processed >= MAX_REPOS_PER_RUN:
            print(f"\nReached limit of {MAX_REPOS_PER_RUN} repos per run, stopping.")
            break

        skip_reason = should_skip(repo, results, state.get("skip_repos", []))
        if skip_reason:
            print(f"  {repo}: skip ({skip_reason})")
            continue

        quality_skip = check_repo_quality(repo)
        if quality_skip:
            print(f"  {repo}: skip ({quality_skip})")
            state.setdefault("skip_repos", []).append(repo)
            save_state(state)
            continue

        print(f"\n--- Processing {repo} ---")

        if not run_scan(repo):
            print(f"  Scan failed for {repo}, skipping upload")
            processed += 1
            continue

        # Reload results after scan
        results = load_results()
        entry = results.get("scanned", {}).get(repo, {})
        matches = entry.get("matches", [])
        pending = [m for m in matches if not m.get("upload_url")]

        if not pending:
            print(f"  No uploadable files, moving on")
            processed += 1
            continue

        time.sleep(UPLOAD_DELAY_SECS)

        if not run_upload(repo):
            print(f"  Upload had errors for {repo}")

        processed += 1

    save_state(state)
    print(f"\n=== Done. Processed {processed} repo(s). Next query: {state['query_index'] + 1}/{len(SEARCH_QUERIES)} ===")


if __name__ == "__main__":
    main()
