# pastebom.com

Shareable interactive PCB BOM viewer. Upload a PCB file, get a link to an interactive viewer.

## Architecture

- **Cargo workspace** with three crates:
  - `crates/pcb-extract` — PCB file parser library + CLI (KiCad, EasyEDA, Eagle, Altium)
  - `crates/server` — Axum web server, handles uploads and serves viewer
  - `crates/viewer` — Yew WASM frontend, built with Trunk
- **Storage**: S3 in production, filesystem locally (`STORAGE_PATH` env var)
- **Viewer assets**: Trunk builds to `crates/viewer/dist/`, server reads from `VIEWER_DIR`

## Dev Script

```bash
./dev.sh start        # build and run on port 8080
./dev.sh start 9000   # build and run on port 9000
./dev.sh stop         # stop the container
./dev.sh status       # check if running and print URL
```

Builds everything inside Docker (`Dockerfile`): server binaries + viewer WASM via Trunk. Storage lives inside the container with no volume mount — every restart is a clean slate.

## Local Development (without Docker)

```bash
# Build viewer WASM
cd crates/viewer && trunk build --release && cd ../..

# Run server
STORAGE_PATH=./localdata cargo run -p pastebom-server
```

Server listens on port 8000 by default (`BIND_ADDR=0.0.0.0:8000`).

## Build & Test Commands

- Build all: `cargo build`
- Build server: `cargo build -p pastebom-server`
- Build viewer: `cd crates/viewer && trunk build --release`
- Test all: `cargo test`
- Test single: `cargo test test_name`
- Lint: `cargo clippy -- -W clippy::all`
- Format: `cargo fmt`

## CI Pre-Push Checklist

```bash
cargo fmt
cargo clippy -- -W clippy::all
cargo test
```

IMPORTANT: Always run `cargo fmt` before committing any code changes!

## Key Environment Variables

| Variable | Default | Description |
|---|---|---|
| `STORAGE_PATH` | `./data` | Filesystem storage root (used when `S3_BUCKET` is not set) |
| `VIEWER_DIR` | `crates/viewer/dist` | Path to built viewer assets |
| `BIND_ADDR` | `0.0.0.0:8000` | Server listen address |
| `S3_BUCKET` | — | Enables S3 storage backend |
| `S3_PREFIX` | — | Key prefix for S3 objects |
| `BASE_URL` | `http://localhost:8000` | Base URL for generated links |

## Code Style Guidelines

- **Imports**: Group in order: std, external crates, local modules. Alphabetize within groups. Avoid wildcard imports.
- **Formatting**: Follow rustfmt defaults. Use trailing commas in multi-line structures.
- **Naming**: snake_case for variables/functions, CamelCase for types/traits, SCREAMING_SNAKE_CASE for constants.
- **Error handling**: Custom error types with thiserror. Use Result with `?` operator.
- **Testing**: NEVER special case testing in production algorithms. Tests should validate real behavior.
- **Comments**: Describe current state only. No references to pre-change conditions.

## Code Editing Guidelines

- **NEVER use sed, awk, or other command-line tools to edit code** — Always use the Edit tool directly
- **NEVER run sudo commands directly** — Always ask the user to run sudo commands manually

## Git Commits

- **NEVER use `git add -A` or `git add .`** — Always add specific files
- **NEVER use force push (`git push -f` or `git push --force`)**
- **NEVER use `git commit --amend`** — Create new commits instead
- **NEVER use `--no-verify` to skip pre-commit hooks** — Fix the underlying issue instead
- **Strongly prefer `git merge` over `git rebase`**
- Do NOT include attribution in commit messages
- Follow the project commit style: short subject line (10 words max), blank line, body with bullet points
- Focus on explaining the WHY (purpose) not just the WHAT (changes)
- Prefer shorter, more focused commits over large monolithic ones
- Always check `git status` before committing to ensure no unwanted files are staged
- After pushing, share the PR link directly (not diff/compare links)
- Prefix branch names with `meawoppl/` and use dashes for spaces

## Shell Commands

- When polling background tasks, sleep for at most 60 seconds between checks
- Prefer `--watch` flags or running commands in background when available

## Versioning

- Version is defined once in the root `Cargo.toml` under `[workspace.package]`
- All crates inherit it via `version.workspace = true`
- The server exposes the version via `/health` and the upload page displays it
- When releasing a new version: bump `version` in root `Cargo.toml` and add a new entry to `CHANGELOG.md`

## Build Notes

- Viewer requires `wasm32-unknown-unknown` target and `trunk` installed
- Server depends on `pcb-extract` as a library
- Trunk config is at `crates/viewer/Trunk.toml` (public_url = `/viewer/`)
