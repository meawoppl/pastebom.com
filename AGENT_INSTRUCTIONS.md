# pastebom.com GitHub Scanner Agent

Automated agent that discovers PCB files on GitHub, uploads them to pastebom.com, and (eventually) opens PRs to add interactive BOM preview links to repository documentation.

## Schedule

Runs every hour via cron.

## Tools

- `discover_and_upload.py` — orchestrator that handles discovery, scanning, and uploading in one invocation
- `scan_github_repo.py` — lower-level tool with `scan` and `upload` subcommands for targeting individual repos
- `gh` CLI — used internally for GitHub search and repo metadata checks
- `pcb-extract` — built from this repo at `target/debug/pcb-extract`, validates PCB files parse correctly before uploading

## Running

### Automatic (steps 1-3 combined)

```bash
python3 discover_and_upload.py
```

Each invocation:
1. Picks the next search query from a 6-query rotation
2. Searches GitHub for up to 30 repos matching that query
3. Filters out forks and already-processed repos
4. For up to 5 new repos: clones, scans for PCB files, validates with `pcb-extract`, uploads to pastebom.com
5. Records all results and advances the query rotation

### Manual (single repo)

```bash
python3 scan_github_repo.py scan owner/repo
python3 scan_github_repo.py upload owner/repo
python3 scan_github_repo.py upload owner/repo --url http://localhost:8000  # test against local server
```

## State files

### `scan_results.json`

Tracks all scanned repos and their PCB files. Schema:

```json
{
  "scanned": {
    "owner/repo": {
      "matches": [
        {
          "path": "board.brd",
          "format": "Eagle",
          "upload_url": "https://pastebom.com/b/{id}",
          "upload_id": "{uuid}",
          "components": 42,
          "upload_error": null
        }
      ],
      "pr_url": "https://github.com/owner/repo/pull/123"
    }
  }
}
```

### `discover_state.json`

Tracks orchestrator state across runs:

```json
{
  "query_index": 2,
  "skip_repos": ["owner/repo-that-was-filtered"]
}
```

`query_index` rotates through the search queries. `skip_repos` caches repos that failed quality checks (forks) so they aren't re-checked via the GitHub API on every run.

## Search queries

The orchestrator rotates through these queries (one per run):

1. `kicad pcb`
2. `eagle pcb`
3. `altium pcb`
4. `gerber pcb`
5. `gdsii`
6. `odb++ pcb`

## Skip conditions

A repo is skipped if any of these are true:

- Already in `scan_results.json` with no matches (no PCB files found)
- All matches already have `upload_url` (fully uploaded)
- In the `skip_repos` list in `discover_state.json`
- Repo is a fork (avoids duplicate work — only target source repos)

## Rate limiting

- At most **5 repos** processed per run
- At most **30 repos** returned per search query
- 2 second delay between scan and upload for each repo
- Progress saved to disk after each individual file upload

## Error handling

- If `pcb-extract` fails on a file, `upload_error` is recorded and remaining files continue
- If upload to pastebom.com fails, error is recorded and remaining files continue
- If clone fails, the repo is skipped entirely
- `scan_github_repo.py` exits non-zero on clone failure; `discover_and_upload.py` catches this and moves on

## Cron setup

```bash
0 * * * * cd /home/meawoppl/repos/pastebom.com && python3 discover_and_upload.py >> discover.log 2>&1
```

Prerequisite: `pcb-extract` must be built first:
```bash
cargo build -p pcb-extract
```

## Step 4 (future): Open PRs to add preview links

For each repo where files were successfully uploaded (has `upload_url` but no `pr_url`):

### 4a: Determine where links belong

Look for documentation files in the repo that reference the PCB files. Priority order:

1. `README.md` in the same directory as the PCB file
2. Root `README.md`
3. Any `.md` file that mentions the PCB filename or board name
4. If no docs exist, create a `BOM.md` in the same directory as the PCB file

### 4b: Generate the link content

For a single board:

```markdown
[View Interactive BOM for filename.brd](https://pastebom.com/b/{upload_id})
```

For multiple boards, use a table:

```markdown
## Interactive BOM Previews

| Board | Components | View |
|-------|-----------|------|
| mini-plasma-driver.brd | 93 | [Open BOM](https://pastebom.com/b/{id}) |
| magic-donut.brd | 36 | [Open BOM](https://pastebom.com/b/{id}) |
```

The SVG thumbnail endpoint at `/b/{id}/thumbnail.svg` can be embedded in PR descriptions for richer previews.

### 4c: Fork, branch, commit, PR

```bash
gh repo fork owner/repo --clone=false
git clone --depth 1 https://github.com/YOUR_BOT_ACCOUNT/repo.git /tmp/repo
cd /tmp/repo
git checkout -b add-pastebom-links

# ... edit documentation files ...

git add specific_files.md
git commit -m "Add interactive BOM preview links"
git push -u origin add-pastebom-links

gh pr create \
  --repo owner/repo \
  --title "Add interactive BOM preview links" \
  --body "$(cat <<'EOF'
## Summary

Adds links to interactive BOM (Bill of Materials) previews hosted on [pastebom.com](https://pastebom.com) for the PCB files in this repository.

These links let anyone view components, designators, and board layout interactively in the browser — no EDA software needed.

| Board | Components | Preview |
|-------|-----------|---------|
| file.brd | 42 | [View BOM](https://pastebom.com/b/xxx) |

## What is pastebom.com?

[pastebom.com](https://pastebom.com) is a free tool that renders interactive BOM viewers for PCB files (KiCad, Eagle, Altium, Gerber, GDSII, ODB++).
EOF
)"
```

### 4d: Record the PR

After creating the PR, update `scan_results.json` to record the PR URL on the repo entry so future runs don't re-open PRs:

```python
results["scanned"]["owner/repo"]["pr_url"] = "https://github.com/owner/repo/pull/123"
```

### 4e: Politeness

- Do not re-open a PR if one was previously closed by the repo owner — record `"pr_declined": true`
- Check for an existing open PR before creating a new one:
  ```bash
  gh pr list --repo owner/repo --head add-pastebom-links --state open
  ```
