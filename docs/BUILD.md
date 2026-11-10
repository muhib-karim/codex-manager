# Build Guide

This guide covers the React UI and the Tauri v2 desktop app. For the project status (the Rust
backend is work in progress and the UI is not wired to it yet) see the [README](../README.md#project-status).

## Requirements

| Tool | Version | Needed for |
|---|---|---|
| Node.js | 20 or newer (CI runs 20 and 22) | UI, tests, Tauri CLI |
| Rust | current stable | Desktop app and backend tests |
| Tauri system dependencies | see below | Desktop app and backend tests |

System dependencies are the standard [Tauri v2 prerequisites](https://tauri.app/start/prerequisites/):

- **Linux (Debian/Ubuntu):**
  ```bash
  sudo apt-get update
  sudo apt-get install -y build-essential curl wget file pkg-config \
    libwebkit2gtk-4.1-dev libssl-dev libxdo-dev \
    libayatana-appindicator3-dev librsvg2-dev
  ```
- **macOS:** Xcode Command Line Tools (`xcode-select --install`).
- **Windows:** Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 11).

## UI only

```bash
npm ci
npm run dev          # http://localhost:1420
npm run lint
npm run typecheck
npm test
npm run build        # output in dist/
```

## Desktop app

```bash
npm ci
npm run tauri dev    # development
npm run tauri build  # bundles in src-tauri/target/release/bundle/
```

No packaged build has been produced or tested yet, so expect rough edges.

## Backend checks

The Rust crate is in `src-tauri/`. `tauri::generate_context!` embeds the built UI, so `dist/`
must exist before the crate compiles:

```bash
npm ci && npm run build
cd src-tauri
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings -A dead_code
cargo test --locked
```

These are the commands the `rust` job in [`ci.yml`](../.github/workflows/ci.yml) runs. The proxy
and usage tests start listeners on `127.0.0.1` with a random port.

## Releases

[`release.yml`](../.github/workflows/release.yml) runs when a `v*` tag is pushed (or is started
manually with an existing tag). It checks that the tag matches the version in `package.json`,
`src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`, builds macOS (arm64 and x64), Linux and
Windows bundles, attaches them to a draft release and publishes it when every build succeeded.
The workflow has not been exercised yet.

To cut a version, update the version in those three files and in `CHANGELOG.md`, commit, then
tag and push the tag.

## Environment variables

| Variable | Description | Default |
|---|---|---|
| `CODEX_MANAGER_MASTER_KEY` | Master key for encrypting stored API keys | A random key generated on first run and kept in `master.key` in the app data directory |
| `RUST_LOG` | Log filter for the backend | `info` |

## Data locations

The backend stores `accounts.db`, `config.json` and `master.key` in the Tauri app data directory
for the identifier `com.aidev2024.codex-manager`:

- Linux: `~/.local/share/com.aidev2024.codex-manager/`
- macOS: `~/Library/Application Support/com.aidev2024.codex-manager/`
- Windows: `%APPDATA%\com.aidev2024.codex-manager\`

## Troubleshooting

- `webkit2gtk-4.1` not found (Linux): install the packages listed above.
- `frontendDist ... doesn't exist` when compiling the backend: run `npm run build` first.
- `The master key does not match this database`: `CODEX_MANAGER_MASTER_KEY` differs from the key
  the database was created with (or `master.key` was replaced). Restore the original key; there is
  no re-encryption tool yet.
