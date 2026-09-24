#!/usr/bin/env python3
"""Fold newly discovered GDSII files into the GDSII loader test corpus.

The pastebom.com-tester discovery agent records GDSII paths it finds on GitHub
in its gdsii_files.json. This script resolves any paths not yet in the corpus
to a commit-pinned blob, groups them by content hash, and appends new cases (or
extra source mirrors for files already catalogued) to
crates/pcb-extract/tests/gdsii_corpus.json. Idempotent: locations already
present (by repo+path) are skipped, so re-running is a no-op.

    ./scripts/update-gdsii-corpus.py --gdsii-files <path/to/gdsii_files.json>

Requires an authenticated `gh` CLI. Exit status is always 0; the last stdout
line is "CHANGED" or "NOCHANGE" so the caller can decide whether to commit.
"""
import argparse
import datetime
import json
import os
import re
import subprocess
import sys
from urllib.parse import quote

DEFAULT_CORPUS = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "crates", "pcb-extract", "tests", "gdsii_corpus.json"
)
SMOKE_MAX = 64 * 1024
STANDARD_MAX = 5 * 1024 * 1024

_LOC_NOTE_RE = re.compile(r"\s*Found byte-identical at \d+ locations \(see sources\)\.")


def human(n):
    f = float(n)
    for u in ["B", "KB", "MB", "GB"]:
        if f < 1024:
            return f"{f:.0f}{u}" if u == "B" else f"{f:.1f}{u}"
        f /= 1024
    return f"{f:.1f}TB"


def tier(size):
    if size <= SMOKE_MAX:
        return "smoke"
    if size <= STANDARD_MAX:
        return "standard"
    return "stress"


def generator_guess(path):
    p = path.lower()
    if p.endswith(".magic.gds"):
        return "magic"
    if p.endswith(".klayout.gds"):
        return "klayout"
    if ".calibre." in p:
        return "calibre"
    if "gdsfactory" in p:
        return "gdsfactory"
    if "openroad" in p or "/flow/" in p or p.startswith("flow/"):
        return "openroad-flow"
    if "klayout" in p:
        return "klayout"
    if "magic" in p:
        return "magic"
    return "unknown"


def pdk_guess(path, repo):
    s = (path + " " + repo).lower()
    for key, name in [
        ("sky130", "sky130"),
        ("gf180", "gf180mcu"),
        ("ihp-sg13g2", "ihp-sg13g2"),
        ("sg13g2", "ihp-sg13g2"),
        ("asap7", "asap7"),
        ("nangate45", "nangate45"),
        ("freepdk45", "freepdk45"),
        ("gf12", "gf12"),
    ]:
        if key in s:
            return name
    return "unknown"


def slugify(s):
    return re.sub(r"-+", "-", re.sub(r"[^a-z0-9]+", "-", s.lower())).strip("-")


def source_of(c):
    """One source pointer from a candidate dict.

    candidate keys: repo, path, commit, commit_date, stars, license
    """
    enc_path = quote(c["path"])
    return {
        "repo": c["repo"],
        "path": c["path"],
        "permalink": f"https://github.com/{c['repo']}/blob/{c['commit']}/{enc_path}",
        "raw_url": f"https://raw.githubusercontent.com/{c['repo']}/{c['commit']}/{enc_path}",
        "commit": c["commit"],
        "commit_date": c["commit_date"],
        "repo_stars": c["stars"],
        "repo_license": c["license"],
    }


def _set_location_note(desc, n):
    desc = _LOC_NOTE_RE.sub("", desc).rstrip()
    if n > 1:
        desc += f" Found byte-identical at {n} locations (see sources)."
    return desc


def make_case(blob_sha, members, date_indexed, used_ids):
    """Build a case (one unique file). `members` are candidate dicts.

    Mutates `used_ids` to reserve the generated id.
    candidate keys: repo, path, size, blob_sha, commit, commit_date,
                    repo_desc, stars, license
    """
    # Primary = most-starred (most canonical), tie-break shortest path then repo.
    members = sorted(members, key=lambda x: (-(x["stars"] or 0), len(x["path"]), x["repo"]))
    primary = members[0]
    repo = primary["repo"]
    owner, name_repo = repo.split("/", 1)
    fname = primary["path"].rsplit("/", 1)[-1]
    stem = fname.rsplit(".", 1)[0]
    cid = slugify(f"{owner}-{name_repo}-{stem}")
    base, n = cid, 2
    while cid in used_ids:
        cid = f"{base}-{n}"
        n += 1
    used_ids.add(cid)

    gen = generator_guess(primary["path"])
    pdk = pdk_guess(primary["path"], repo)
    sources = [source_of(m) for m in members]

    desc = f"GDSII layout `{fname}` ({human(primary['size'])}) from {repo}."
    if primary.get("repo_desc"):
        desc += f" Repo: {primary['repo_desc'].strip()}."
    desc += f" Path: {primary['path']}."
    desc += f" Generator guess: {gen}; PDK guess: {pdk}."
    desc = _set_location_note(desc, len(sources))

    return {
        "id": cid,
        "name": f"{name_repo}: {fname}",
        "description": desc,
        "blob_sha": blob_sha,
        "size_bytes": primary["size"],
        "tier": tier(primary["size"]),
        "generator_guess": gen,
        "pdk_guess": pdk,
        "date_indexed": date_indexed,
        "source_count": len(sources),
        # Convenience copy of the primary location; `sources` lists every mirror.
        "repo": primary["repo"],
        "path": primary["path"],
        "permalink": sources[0]["permalink"],
        "raw_url": sources[0]["raw_url"],
        "commit": primary["commit"],
        "commit_date": primary["commit_date"],
        "repo_stars": primary["stars"],
        "repo_license": primary["license"],
        "sources": sources,
    }


def append_sources(case, members):
    """Add newly found locations (candidate dicts) to an existing case.

    Append-only: the established primary (sources[0], id, raw_url) never changes,
    so case ids and links stay stable across discovery runs. Returns # added.
    """
    have = {(s["repo"], s["path"]) for s in case["sources"]}
    added = 0
    for m in members:
        key = (m["repo"], m["path"])
        if key in have:
            continue
        case["sources"].append(source_of(m))
        have.add(key)
        added += 1
    if added:
        case["source_count"] = len(case["sources"])
        case["description"] = _set_location_note(case["description"], case["source_count"])
    return added


def recompute(manifest):
    """Refresh derived counts after cases change."""
    cases = manifest["cases"]
    tc = {}
    for c in cases:
        tc[c["tier"]] = tc.get(c["tier"], 0) + 1
    manifest["tier_counts"] = tc
    pointers = sum(c["source_count"] for c in cases)
    sel = manifest.setdefault("selection", {})
    sel["unique_files"] = len(cases)
    sel["resolvable_source_pointers"] = pointers
    sel["total_discovered_paths"] = max(sel.get("total_discovered_paths", 0), pointers)


def gh_json(args, timeout=90):
    r = subprocess.run(["gh", "api", *args], capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0 or not r.stdout.strip():
        return None
    try:
        return json.loads(r.stdout)
    except json.JSONDecodeError:
        return None


def fetch_repo(repo):
    """Return (commit_sha, commit_date, info, blobs{path:{size,sha}}) or None."""
    info = gh_json([f"repos/{repo}", "--jq",
                    "{desc:.description, stars:.stargazers_count, license:(.license.spdx_id // null)}"]) or {}
    commit = gh_json([f"repos/{repo}/commits/HEAD", "--jq", "{sha:.sha, date:.commit.committer.date}"]) or {}
    if not commit.get("sha"):
        return None
    tree = gh_json([f"repos/{repo}/git/trees/HEAD?recursive=1"])
    blobs = {}
    if tree:
        blobs = {e["path"]: {"size": e.get("size"), "sha": e.get("sha")}
                 for e in tree.get("tree", []) if e.get("type") == "blob"}
    return commit["sha"], commit.get("date"), info, blobs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", default=DEFAULT_CORPUS, help="gdsii_corpus.json to update in place")
    ap.add_argument("--gdsii-files", required=True, help="discovered GDSII paths (pastebom.com-tester gdsii_files.json)")
    args = ap.parse_args()

    date_today = datetime.date.today().isoformat()
    discovered = json.load(open(args.gdsii_files))["repos"]
    manifest = json.load(open(args.corpus))
    cases = manifest["cases"]

    # Index existing state.
    have_loc = {(s["repo"], s["path"]) for c in cases for s in c["sources"]}
    by_blob = {c["blob_sha"]: c for c in cases}
    used_ids = {c["id"] for c in cases}

    # Which repos have at least one path not yet in the corpus?
    todo = {repo: [p for p in paths if (repo, p) not in have_loc]
            for repo, paths in discovered.items()}
    todo = {r: ps for r, ps in todo.items() if ps}
    if not todo:
        print("No new GDSII locations vs corpus.")
        print("NOCHANGE")
        return

    # Fetch metadata only for repos with new paths, build candidates.
    candidates = []
    for i, (repo, paths) in enumerate(todo.items(), 1):
        res = fetch_repo(repo)
        if not res:
            print(f"  [{i}/{len(todo)}] {repo}: metadata fetch failed, skipping", file=sys.stderr)
            continue
        commit, commit_date, info, blobs = res
        for p in paths:
            b = blobs.get(p)
            if not b or b.get("size") is None or not b.get("sha"):
                continue  # gone / LFS pointer / unresolved
            candidates.append({
                "repo": repo, "path": p, "size": b["size"], "blob_sha": b["sha"],
                "commit": commit, "commit_date": commit_date,
                "repo_desc": info.get("desc"), "stars": info.get("stars"), "license": info.get("license"),
            })
        print(f"  [{i}/{len(todo)}] {repo}: {len(paths)} new path(s)", file=sys.stderr)

    # Group new candidates by content hash and merge.
    groups, order = {}, []
    for c in candidates:
        groups.setdefault(c["blob_sha"], []).append(c)
        if c["blob_sha"] not in order:
            order.append(c["blob_sha"])

    new_files = new_pointers = 0
    for h in order:
        members = groups[h]
        if h in by_blob:
            new_pointers += append_sources(by_blob[h], members)
        else:
            case = make_case(h, members, date_today, used_ids)
            cases.append(case)
            by_blob[h] = case
            new_files += 1
            new_pointers += case["source_count"]

    if new_files == 0 and new_pointers == 0:
        print("New paths resolved to files already in corpus (no change).")
        print("NOCHANGE")
        return

    recompute(manifest)
    manifest["generated_date"] = date_today
    json.dump(manifest, open(args.corpus, "w"), indent=2)

    print(f"Added {new_files} new file(s) and {new_pointers} new source pointer(s).")
    print(f"Corpus now: {len(cases)} unique files, tiers={manifest['tier_counts']}.")
    print("CHANGED")


if __name__ == "__main__":
    main()
