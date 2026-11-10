# AGENTS.md - Codex Manager

## Project Overview

**Codex Manager** is a desktop application for managing multiple OpenAI API accounts with routing,
usage monitoring and a local OpenAI-compatible proxy. Built with Tauri v2 + React + TypeScript + Rust.

**Version**: 0.3.0 (tagged, never built or released)
**Status**: work-in-progress prototype (see below)
**License**: MIT
**Repository**: https://github.com/muhib-karim/codex-manager
**Support**: https://ko-fi.com/ai_dev_2024

### Current state (read this first)

- The React UI works in the browser and is covered by lint, type-check, tests and build in CI.
- The Rust backend is work in progress. It is compiled, linted and tested **only in CI** (the `rust`
  job). Do not assume a change to it compiles until that job is green.
- **The UI is not wired to the backend.** Nothing calls `invoke()`. Accounts live in memory in
  `src/stores/useAccountStore.ts`; settings live in `localStorage`.
- No packaged build or release exists.

---

## Architecture

```
Codex Manager
├── Frontend (React 19 + TypeScript + Vite)        src/
│   ├── Dashboard (stats, best account)
│   ├── Accounts (add, import, export, batch actions)
│   ├── Settings (general, proxy, notifications, appearance)
│   └── About
│
├── Backend (Rust + Tauri v2)                       src-tauri/src/
│   ├── commands.rs   24 Tauri commands
│   ├── routing.rs    4 strategies + circuit breaker
│   ├── storage.rs    SQLite, API keys encrypted (AES-256-GCM, Argon2id)
│   ├── usage.rs      OpenAI billing usage client (no background polling yet)
│   ├── proxy.rs      Axum pass-through proxy
│   ├── config.rs     config.json, data dir, master key
│   └── tray.rs       system tray
│
└── Distribution (untested)
    └── .github/workflows/release.yml: macOS arm64/x64, Linux, Windows bundles on a v* tag
```

---

## Tech Stack

### Frontend
- **React 19**, **TypeScript**, **Vite**
- **Tailwind CSS 3** + **DaisyUI** - styling
- **Zustand** - state
- **Lucide React** - icons
- **Vitest** - unit tests; **ESLint** (flat config) - lint
- Declared but not used yet: i18next, Recharts, framer-motion, dnd-kit, Radix

### Backend
- **Rust** (edition 2021), **Tauri v2**, **Tokio**
- **Axum 0.7** + **reqwest 0.12** - proxy
- **rusqlite** (bundled SQLite)
- **aes-gcm** + **argon2** - encryption of stored API keys

### Tools
- **GitHub Actions** - CI (`ci.yml`) and release (`release.yml`)

---

## File Structure

```
codex-manager/
├── .github/workflows/
│   ├── ci.yml                   # frontend, rust and gitleaks jobs
│   └── release.yml              # tag-triggered Tauri release
├── docs/
│   ├── API.md                   # proxy behaviour
│   ├── BUILD.md                 # build and check instructions
│   ├── ARCHITECTURE.md          # OUTDATED (earlier CLI design)
│   └── OPENCODE_INTEGRATION.md  # OUTDATED (earlier CLI design)
├── public/                      # icon and banner
├── src/                         # React frontend
│   ├── components/{accounts,common,dashboard,layout}/
│   ├── pages/{Dashboard,Accounts,Settings,About}.tsx
│   ├── stores/{useAccountStore,useConfigStore}.ts
│   ├── hooks/useTauri.ts        # generic invoke hook (unused so far)
│   ├── lib/backend.ts           # typed contract for the Tauri commands
│   ├── types/{account,config}.ts
│   ├── utils/                   # pure helpers: quota, accountList, importAccounts, settings, format, cn
│   ├── App.tsx, main.tsx, index.css
├── src-tauri/
│   ├── src/{lib,main,commands,config,models,proxy,routing,storage,tray,usage}.rs
│   ├── icons/                   # 32x32, 128x128, 128x128@2x (RGBA PNG), icon.ico
│   ├── capabilities/default.json
│   ├── Cargo.toml, Cargo.lock, build.rs, tauri.conf.json
├── tests/                       # Vitest tests
├── eslint.config.js
├── CHANGELOG.md, README.md, LICENSE, AGENTS.md
├── package.json, package-lock.json
└── vite.config.ts, tsconfig.json, tailwind.config.js, postcss.config.js, components.json, index.html
```

---

## Key Commands

### Frontend
```bash
npm ci
npm run dev          # http://localhost:1420
npm run lint         # ESLint, warnings fail
npm run typecheck
npm test             # Vitest
npm run build
```

### Backend (what CI runs, from src-tauri/, after `npm run build`)
```bash
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings -A dead_code
cargo test --locked
```
`-A dead_code` exists only because the UI does not call the backend yet. Remove it when it does.

### Desktop app
```bash
npm run tauri dev
npm run tauri build
```

---

## Tauri Commands

All commands are `pub async fn` in `src-tauri/src/commands.rs` and are registered in `lib.rs` by
module path (`commands::add_account`). Tauri requires commands to live outside the crate root.
`src/lib/backend.ts` mirrors names, arguments and results; `tests/backendContract.test.ts` fails
when the two sides differ.

| Group | Commands |
|---|---|
| Accounts | `add_account`, `update_account`, `remove_account`, `get_account`, `list_accounts`, `list_account_statuses`, `toggle_account_enabled` |
| Usage | `get_account_usage`, `refresh_all_usage`, `refresh_account_usage` |
| Routing | `get_routing_stats`, `set_routing_strategy`, `clear_routing_sessions` |
| Proxy | `start_proxy_server`, `stop_proxy_server`, `get_proxy_status` |
| Import/export | `export_accounts`, `import_accounts`, `validate_api_key` |
| Config | `load_app_config`, `save_app_config`, `update_proxy_configuration`, `update_routing_configuration`, `get_data_directory` |

Rules:
- Top-level `invoke()` argument keys are camelCase (`apiKey` for the Rust parameter `api_key`);
  fields inside struct arguments and results keep their Rust names (snake_case).
- Commands return accounts **redacted** (`Account::redacted`, masked key). Only `export_accounts`
  returns API keys.

---

## Routing Strategies

Implemented in `src-tauri/src/routing.rs`; wire names are snake_case (`RoutingStrategy` in `models.rs`).

1. **least_utilized** (default): lowest utilization, where utilization is the tightest of spend
   against the provider hard limit, the account's monthly limit and the account's daily limit.
   Ties go to the account selected longest ago.
2. **round_robin**: cycles through eligible accounts.
3. **priority**: highest `priority`; ties go to the account selected longest ago.
4. **sticky**: session id = hash of the first chat message; a session stays on its account. No
   session means `least_utilized`.

Eligible = enabled, within limits, model in scope, circuit not open. The circuit opens after 3
consecutive account failures (401, 403, 429, 5xx, connection error) for 60 s, then half-opens.

---

## Security Model

- API keys: AES-256-GCM, random 96-bit nonce per value, stored as base64(nonce || ciphertext).
- Key derivation: Argon2id from the master key and a random salt kept in the `metadata` table.
  The salt must never be regenerated for an existing database.
- Master key: `CODEX_MANAGER_MASTER_KEY`, otherwise a random key in `master.key` (0600) in the app
  data directory. There is no built-in default key and there must never be one.
- Proxy: binds to `127.0.0.1:8080` by default, bearer key (`sk-codex-manager` by default), no CORS,
  64 MiB body limit, `/health` and `/healthz` are open.
- Never log or print API keys. `Account` and `RoutingDecision` have manual `Debug` impls that mask
  or omit them; keep it that way.

---

## Configuration

Backend files live in the Tauri app data directory for `com.aidev2024.codex-manager`
(Linux `~/.local/share/...`, macOS `~/Library/Application Support/...`, Windows `%APPDATA%\...`):
`accounts.db`, `config.json`, `master.key`.

`config.json` (defaults):
```json
{
  "proxy": { "bind_addr": "127.0.0.1:8080", "api_key": "sk-codex-manager", "openai_base_url": "https://api.openai.com" },
  "routing": { "strategy": "least_utilized", "min_request_interval_ms": 100 }
}
```

UI settings (theme, language, proxy form, notifications, appearance) are separate and live in
`localStorage` under `codex-manager-config`; see `src/types/config.ts` and `src/utils/settings.ts`.

---

## GitHub Actions

- **ci.yml** (push to `main`, pull requests into `main`): `frontend` (Node 20 and 22: lint,
  type-check, tests, build, production audit), `rust` (check, clippy, test), `gitleaks` (full history).
- **release.yml** (`v*` tag or manual dispatch with a tag): checks the tag against the manifests,
  builds and uploads bundles, publishes the draft. Untested.
- Actions are pinned to commit SHAs with the tag in a comment. Keep new actions pinned.

---

## External APIs

- Usage: `GET /v1/dashboard/billing/usage?start_date&end_date` (end date exclusive) and
  `GET /v1/dashboard/billing/subscription`. These are not part of OpenAI's documented API and may
  refuse standard API keys.
- Key validation: `GET /v1/models`.
- Local proxy: `http://127.0.0.1:8080/v1`, health at `/health`.

---

## Common Tasks

### Adding a New Page
1. Create `src/pages/NewPage.tsx`
2. Add the route in `src/App.tsx`
3. Add the navigation item in `src/components/layout/Layout.tsx`

### Adding a Tauri Command
1. Add a `#[tauri::command] pub async fn` to `src-tauri/src/commands.rs`
2. Register it in `src-tauri/src/lib.rs` as `commands::new_command`
3. Add it to `BackendCommandMap` and `BACKEND_COMMANDS` in `src/lib/backend.ts`
   (the contract test fails until all three agree)
4. Put testable logic in a plain function and unit-test it in the same file

### Adding a New Routing Strategy
1. Add the variant to `RoutingStrategy` in `src-tauri/src/models.rs`
2. Handle it in `RoutingEngine::resolve_account` in `src-tauri/src/routing.rs`, with tests
3. Add it to `SCHEDULING_MODES` in `src/types/config.ts` and the label map in `src/pages/Settings.tsx`
   (the contract test compares the two lists)

### Creating a Release
1. Update the version in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `CHANGELOG.md`
2. Commit, tag `vX.Y.Z` and push the tag; `release.yml` rejects a tag that does not match the manifests

---

## Testing

- Frontend: `npm test` (Vitest, node environment, no DOM). Put UI logic in pure helpers under
  `src/utils/` so it can be tested.
- Backend: `#[cfg(test)]` modules in each file. Proxy and usage tests start real listeners on
  `127.0.0.1:0` and a mock upstream.
- Every bug fix comes with a regression test.

### API Testing (once the proxy runs)
```bash
curl http://localhost:8080/health

curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-codex-manager" \
  -H "Content-Type: application/json" \
  -d '{"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "Hello"}]}'
```

---

## Troubleshooting

- **Linux**: "webkit2gtk not found" -> install `libwebkit2gtk-4.1-dev`; "lssl not found" -> `libssl-dev`
- **macOS**: "cc not found" -> `xcode-select --install`
- **Windows**: "link.exe not found" -> install Visual Studio Build Tools
- **Backend compile**: "frontendDist ... doesn't exist" -> run `npm run build` first
- **Backend start**: "The master key does not match this database" -> the master key changed

---

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CODEX_MANAGER_MASTER_KEY` | Master key for encrypting stored API keys | Random key in `master.key` |
| `RUST_LOG` | Rust logging filter | info |
| `RUST_BACKTRACE` | Enable backtraces | 0 |

---

## Resources

- [README.md](README.md), [docs/BUILD.md](docs/BUILD.md), [docs/API.md](docs/API.md), [CHANGELOG.md](CHANGELOG.md)
- [Tauri v2 docs](https://v2.tauri.app/), [React docs](https://react.dev), [Rust Book](https://doc.rust-lang.org/book/), [Tailwind CSS](https://tailwindcss.com)
- Issues: https://github.com/muhib-karim/codex-manager/issues
- Ko-fi: https://ko-fi.com/ai_dev_2024

---

## License

MIT License - See [LICENSE](LICENSE) file

Copyright (c) 2026 Muhib Al Karim

---

## Notes for Future Agents

1. **This is a Tauri v2 application** - Use Tauri v2 APIs, not v1
2. **React 19** - Use modern React patterns
3. **Rust backend** - All heavy operations in Rust
4. **Encrypted storage** - Never store credentials in plaintext
5. **GitHub Actions** - CI gates every change; release builds run on tag push
6. **Version consistency** - Keep versions synced across all files
7. **Security first** - Review all code for security issues
8. **Cross-platform** - Test on Windows, macOS, Linux
9. **Documentation** - Update docs when adding features; do not describe features that do not exist
10. **Support Ko-fi** - Include support link in user-facing docs
11. **Be honest about status** - The backend is WIP and the UI is not wired to it; say so
12. **Keep Rust changes conservative** - Small, well-known API calls; the backend is verified in CI

---

**Last Updated**: 2026-11-10
**Version**: 0.3.0
**Status**: Work-in-progress prototype
