#!/usr/bin/env python3
"""Scan a GitHub repository for files that could be uploaded to pastebom.com."""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import urllib.request
import urllib.error
from pathlib import Path

RESULTS_FILE = "scan_results.json"
PASTEBOM_URL = "https://pastebom.com"
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PCB_EXTRACT_BIN = os.path.join(SCRIPT_DIR, "target", "debug", "pcb-extract")


def load_results(path: str) -> dict:
    if os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    return {"scanned": {}}


def save_results(path: str, data: dict) -> None:
    with open(path, "w") as f:
        json.dump(data, f, indent=2)


def is_pcb_file(filepath: str) -> str | None:
    """Return the detected format name if the file looks like a PCB file, else None."""
    lower = filepath.lower()

    if lower.endswith(".kicad_pcb"):
        return "KiCad"
    if lower.endswith(".brd") or lower.endswith(".fbrd"):
        return "Eagle"
    if lower.endswith(".pcbdoc"):
        return "Altium"
    if lower.endswith(".gds") or lower.endswith(".gds2"):
        return "GDSII"
    if lower.endswith(".tgz") or lower.endswith(".tar.gz"):
        return "ODB++ (probable)"
    if lower.endswith(".zip"):
        return "Gerber/ODB++ (zip)"
    return None


def try_extract(filepath: str) -> bool:
    """Run pcb-extract locally on a file. Returns True if it parses successfully."""
    if not os.path.exists(PCB_EXTRACT_BIN):
        print(f"  pcb-extract not found at {PCB_EXTRACT_BIN}", file=sys.stderr)
        print("  Run 'cargo build -p pcb-extract' first.", file=sys.stderr)
        return False

    result = subprocess.run(
        [PCB_EXTRACT_BIN, filepath],
        capture_output=True,
        timeout=60,
    )
    return result.returncode == 0


def upload_file(filepath: str, base_url: str) -> dict | None:
    """Upload a file to pastebom.com. Returns the JSON response or None on failure."""
    filename = os.path.basename(filepath)
    boundary = "----PasteBomUploadBoundary"

    with open(filepath, "rb") as f:
        file_data = f.read()

    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: application/octet-stream\r\n"
        f"\r\n"
    ).encode() + file_data + f"\r\n--{boundary}--\r\n".encode()

    req = urllib.request.Request(
        f"{base_url}/upload",
        data=body,
        headers={
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        error_body = e.read().decode(errors="replace")
        print(f"  Upload failed ({e.code}): {error_body}", file=sys.stderr)
        return None
    except urllib.error.URLError as e:
        print(f"  Upload failed: {e.reason}", file=sys.stderr)
        return None


def scan_repo(repo: str, results_path: str) -> None:
    """Clone and scan a repo for PCB files."""
    results = load_results(results_path)

    if repo in results["scanned"]:
        prev = results["scanned"][repo]
        if prev["matches"]:
            print(f"Already scanned {repo} — {len(prev['matches'])} file(s) found previously.")
        else:
            print(f"Already scanned {repo} — no PCB files found.")
        return

    # Normalize repo to a clone URL
    if os.path.isdir(repo):
        clone_url = os.path.abspath(repo)
    elif not repo.startswith("http") and not repo.startswith("git@"):
        clone_url = f"https://github.com/{repo}.git"
    else:
        clone_url = repo

    print(f"Cloning {clone_url} ...")

    with tempfile.TemporaryDirectory() as tmpdir:
        result = subprocess.run(
            ["git", "clone", "--depth", "1", clone_url, tmpdir],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Failed to clone: {result.stderr.strip()}", file=sys.stderr)
            sys.exit(1)

        print("Scanning for PCB files ...")
        matches = []
        for root, _dirs, files in os.walk(tmpdir):
            for name in files:
                full = os.path.join(root, name)
                rel = os.path.relpath(full, tmpdir)
                fmt = is_pcb_file(name)
                if fmt:
                    matches.append({"path": rel, "format": fmt})

    results["scanned"][repo] = {"matches": matches}
    save_results(results_path, results)

    if matches:
        print(f"Found {len(matches)} candidate file(s):")
        for m in matches:
            print(f"  {m['path']}  ({m['format']})")
    else:
        print("No PCB files found.")


def upload_repo(repo: str, results_path: str, base_url: str) -> None:
    """Extract and upload PCB files from a previously scanned repo."""
    results = load_results(results_path)

    if repo not in results["scanned"]:
        print(f"{repo} has not been scanned yet. Run 'scan' first.")
        sys.exit(1)

    entry = results["scanned"][repo]
    matches = entry["matches"]
    if not matches:
        print(f"No PCB files found in {repo}. Nothing to upload.")
        return

    already_uploaded = [m for m in matches if m.get("upload_url")]
    if len(already_uploaded) == len(matches):
        print(f"All {len(matches)} file(s) from {repo} already uploaded:")
        for m in matches:
            print(f"  {m['path']} -> {m['upload_url']}")
        return

    # Normalize repo to a clone URL
    if os.path.isdir(repo):
        clone_url = os.path.abspath(repo)
    elif not repo.startswith("http") and not repo.startswith("git@"):
        clone_url = f"https://github.com/{repo}.git"
    else:
        clone_url = repo

    print(f"Cloning {clone_url} for upload ...")

    with tempfile.TemporaryDirectory() as tmpdir:
        result = subprocess.run(
            ["git", "clone", "--depth", "1", clone_url, tmpdir],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"Failed to clone: {result.stderr.strip()}", file=sys.stderr)
            sys.exit(1)

        uploaded = 0
        skipped = 0
        failed = 0

        for match in matches:
            path = match["path"]
            full_path = os.path.join(tmpdir, path)

            if match.get("upload_url"):
                print(f"  {path}: already uploaded -> {match['upload_url']}")
                skipped += 1
                continue

            if not os.path.exists(full_path):
                print(f"  {path}: file not found, skipping")
                match["upload_error"] = "file not found"
                failed += 1
                continue

            print(f"  {path}: extracting locally ...", end=" ", flush=True)
            if not try_extract(full_path):
                print("FAILED (pcb-extract could not parse)")
                match["upload_error"] = "pcb-extract failed"
                failed += 1
                continue
            print("OK", end=" ", flush=True)

            print("-> uploading ...", end=" ", flush=True)
            resp = upload_file(full_path, base_url)
            if resp and "url" in resp:
                print(f"OK ({resp['components']} components)")
                match["upload_url"] = resp["url"]
                match["upload_id"] = resp["id"]
                match["components"] = resp["components"]
                uploaded += 1
            else:
                print("FAILED")
                match["upload_error"] = "upload failed"
                failed += 1

            # Save after each upload so progress isn't lost
            save_results(results_path, results)

    save_results(results_path, results)
    print(f"\nDone: {uploaded} uploaded, {skipped} already done, {failed} failed")


def main():
    parser = argparse.ArgumentParser(
        description="Scan a GitHub repo for files uploadable to pastebom.com"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # scan subcommand
    scan_parser = subparsers.add_parser("scan", help="Scan a repo for PCB files")
    scan_parser.add_argument(
        "repo",
        help="GitHub repo in owner/name format, or a full git URL",
    )
    scan_parser.add_argument(
        "--results",
        default=RESULTS_FILE,
        help=f"Path to the JSON results file (default: {RESULTS_FILE})",
    )

    # upload subcommand
    upload_parser = subparsers.add_parser(
        "upload", help="Extract and upload PCB files from a scanned repo"
    )
    upload_parser.add_argument(
        "repo",
        help="GitHub repo in owner/name format (must be scanned first)",
    )
    upload_parser.add_argument(
        "--results",
        default=RESULTS_FILE,
        help=f"Path to the JSON results file (default: {RESULTS_FILE})",
    )
    upload_parser.add_argument(
        "--url",
        default=PASTEBOM_URL,
        help=f"pastebom.com base URL (default: {PASTEBOM_URL})",
    )

    args = parser.parse_args()

    if args.command == "scan":
        scan_repo(args.repo, args.results)
    elif args.command == "upload":
        upload_repo(args.repo, args.results, args.url)


if __name__ == "__main__":
    main()
