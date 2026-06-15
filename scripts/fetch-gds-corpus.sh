#!/usr/bin/env bash
#
# Download GDSII corpus files for the parse harness, naming each by its blob SHA
# and verifying integrity against the catalog.
#
#   GDS_CORPUS_DIR=/tmp/gds ./scripts/fetch-gds-corpus.sh [tier]
#
# tier: smoke (default) | standard | stress | all
#
# Then run the harness:
#   GDS_CORPUS_DIR=/tmp/gds cargo test -p pcb-extract --test gdsii_corpus -- --ignored --nocapture
#
set -euo pipefail

TIER="${1:-smoke}"
DIR="${GDS_CORPUS_DIR:?set GDS_CORPUS_DIR to a download directory}"
CORPUS="$(cd "$(dirname "$0")/.." && pwd)/crates/pcb-extract/tests/gdsii_corpus.json"

command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
mkdir -p "$DIR"

ok=0 bad=0 miss=0
while IFS=$'\t' read -r sha url; do
  out="$DIR/$sha.gds"
  [ -f "$out" ] && { ok=$((ok + 1)); continue; }
  if curl -fsSL --retry 2 -o "$out.tmp" "$url"; then
    got="$(git hash-object "$out.tmp")"
    if [ "$got" = "$sha" ]; then
      mv "$out.tmp" "$out"
      ok=$((ok + 1))
    else
      # Wrong content (redirect / 404 page / LFS pointer) — don't keep it.
      echo "BAD SHA  $sha (got $got)  $url" >&2
      rm -f "$out.tmp"
      bad=$((bad + 1))
    fi
  else
    echo "MISS     $sha  $url" >&2
    rm -f "$out.tmp"
    miss=$((miss + 1))
  fi
done < <(jq -r --arg tier "$TIER" \
  '.cases[] | select($tier == "all" or .tier == $tier) | "\(.blob_sha)\t\(.raw_url)"' \
  "$CORPUS")

echo "fetched $ok ok, $bad bad-sha, $miss missing -> $DIR"
