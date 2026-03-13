# Cron Agent: PCB Discovery Pipeline

Agent that runs every ~2 hours to discover PCB files on GitHub, upload them to pastebom.com, commit progress, and file issues for parse failures.

## Prerequisites

- Working directory: the pastebom.com repo checkout
- `pcb-extract` binary built: `cargo build -p pcb-extract`
- `gh` CLI authenticated
- `git` configured with push access to origin

## Steps

### 1. Pull latest main

```bash
git checkout main
git pull origin main
```

### 2. Run the discovery pipeline

```bash
python3 discover_and_upload.py
```

This runs one search query (rotating through 6), scans up to 5 new repos, and uploads any PCB files that parse successfully. All results are saved to `scan_results.json` and `discover_state.json`.

### 3. File issues for pcb-extract failures

After the pipeline runs, check `scan_results.json` for any matches where `upload_error` is `"pcb-extract failed"`. For each one that doesn't already have an issue filed:

```bash
# Find failures
python3 -c "
import json
d = json.load(open('scan_results.json'))
for repo, info in d['scanned'].items():
    for m in info.get('matches', []):
        if m.get('upload_error') == 'pcb-extract failed' and not m.get('issue_filed'):
            print(f'{repo} | {m[\"path\"]} | {m[\"format\"]}')
"
```

For each failure, open an issue on the pastebom.com repo:

```bash
gh issue create \
  --repo meawoppl/pastebom.com \
  --title "pcb-extract fails to parse: filename.brd" \
  --body "$(cat <<EOF
## File

- **Repo**: https://github.com/owner/repo
- **Path**: [\`path/to/file.brd\`](https://github.com/owner/repo/blob/main/path/to/file.brd)
- **Format**: Eagle

## Context

The discovery pipeline found this file but \`pcb-extract\` failed to parse it. This file should either be supported or explicitly rejected with a clear error.

Found by the automated PCB discovery agent.
EOF
)" --label "bug"
```

After filing, update the match entry in `scan_results.json` to record the issue URL:

```python
match["issue_filed"] = "https://github.com/meawoppl/pastebom.com/issues/XX"
```

This prevents duplicate issues on future runs.

### 4. Commit and PR the progress

If `scan_results.json` or `discover_state.json` changed:

```bash
git checkout -b meawoppl/scanner-progress-YYYY-MM-DD-HH
git add scan_results.json discover_state.json
git commit -m "Update scanner progress"
git push -u origin meawoppl/scanner-progress-YYYY-MM-DD-HH
gh pr create \
  --title "Update scanner progress" \
  --body "$(cat <<'EOF'
Automated update from the PCB discovery pipeline.
EOF
)"
gh pr merge --auto --squash
```

Use the current date and hour in the branch name to avoid conflicts.

### 5. Return to main

```bash
git checkout main
```

## Handling edge cases

- **No changes**: If the pipeline found nothing new (all repos already scanned), skip the commit/PR step.
- **Clone failures**: The pipeline handles these internally — it skips the repo and moves on.
- **Upload failures**: Recorded as `upload_error` in `scan_results.json`. These are not filed as issues since they're server-side, not parser bugs.
- **pcb-extract failures**: These ARE filed as issues because they represent files the parser should handle.
- **Duplicate issues**: Always check `issue_filed` field before creating. If it's set, skip.
- **Branch conflicts**: The date-hour branch name should avoid collisions. If a PR is still open from a previous run, just push to that branch instead of creating a new one.

## Issue labeling

When creating issues for parse failures, use the `bug` label. Include:
- A direct link to the file on GitHub (use the default branch, usually `main` or `master`)
- The detected format so it's clear which parser is involved
- The repo name for context

## Schedule

Run every 2 hours. Each run processes one search query and up to 5 repos, so over 12 hours all 6 query categories get covered and up to 30 repos get processed.
