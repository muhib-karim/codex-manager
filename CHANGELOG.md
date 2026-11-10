# Changelog

All notable changes to Codex Manager will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Rust unit tests in every backend module (storage and encryption, routing strategies and circuit
  breaker, proxy against a mock upstream, usage math, command helpers, master key handling).
- Vitest regression tests for quota math, list/pagination/selection, account import, settings and
  theme, the account store, and a contract test that compares `src/lib/backend.ts` with the
  commands and routing strategies in the Rust sources (17 -> 65 tests).
- `src/lib/backend.ts`: typed map of the Tauri commands and their argument shapes.
- ESLint (flat config) with an `npm run lint` script.
- CI: `rust` job (`cargo check`, `clippy`, `test` with `--locked`), `gitleaks` job over the full
  history, lint step, Node 20 and 22, production dependency audit; actions pinned to commit SHAs,
  timeouts, concurrency group, read-only token.
- `typecheck` and `test` npm scripts, MIT `LICENSE` file and `docs/banner.svg` (earlier in this cycle).

### Changed
- Backend commands live in `src-tauri/src/commands.rs` and return accounts with the API key masked;
  only `export_accounts` returns keys. `refresh_all_usage` returns a flat list of results.
- The proxy forwards any path with its method, query string, body and content type and returns the
  upstream status, streamed body and selected headers. The hard-coded model list is gone.
- `/health` and `/healthz` no longer need the proxy key; the proxy sends no CORS headers.
- Master key: `CODEX_MANAGER_MASTER_KEY`, otherwise a random per-install key file. There is no
  built-in default key any more.
- Scheduling modes in the UI are the four strategies the backend implements
  (`least_utilized`, `round_robin`, `priority`, `sticky`); stored legacy values are mapped.
- Release workflow: installs frontend dependencies, uses the dispatch tag, checks the tag against
  the manifests, awaits publishing, minimal permissions, pinned actions.
- README, `docs/BUILD.md` and `docs/API.md` rewritten to match the code; `docs/ARCHITECTURE.md` and
  `docs/OPENCODE_INTEGRATION.md` marked as outdated.

### Fixed
- Backend did not compile: duplicate `models` module, missing `aes-gcm`/`argon2` dependencies,
  `chrono` without `serde`, a store API from a plugin that was not a dependency, fields and methods
  that did not exist, non-RGBA bundle icons.
- The encryption salt was regenerated on every start, so stored API keys could not be decrypted
  after a restart. The salt is now stored with the database and a wrong master key is detected.
- `set_routing_strategy` did nothing; round-robin panicked (`blocking_read` in async code);
  least-utilized ignored per-account limits and always picked the first account; requests without a
  session were pinned to one account; a tripped circuit breaker could stay open forever; 401/403/429
  did not count against an account.
- Proxy forced every request to `POST` with a JSON body, dropped the query string and replaced the
  upstream status with `200`; the proxy start command asked for state that was never registered.
- Usage: today's spend was excluded (exclusive end date), `daily_usage` was never filled, and a
  failed refresh was stored as a zero-usage snapshot.
- A monthly limit was dropped unless a daily limit was also set.
- The app window was created hidden and never shown; the tray icon was never created.
- UI: unknown quota counted as 0 % (or as 50000 % when only the total was missing); "best account"
  could be an exhausted one; "Last used" always read "Reset"; the accounts page showed an empty
  page after filtering; deleted accounts stayed selected and batch actions hit hidden accounts;
  search matched API keys; the list view showed 14 characters of each key; import accepted junk and
  duplicates; port and refresh interval accepted any number; the theme needed two clicks from
  "system" and the boot script read a key nothing wrote; the first account was current but not active.
- `src/pages/About.tsx` JSX, the account store types and unused imports (earlier in this cycle).

### Removed
- The standalone CLI/TUI crate at the repository root (`Cargo.toml`, `src/**/*.rs`): a duplicate of
  the backend that never compiled.
- `PROJECT_SUMMARY.md`, `RELEASE_CHECKLIST.md`, dead backend stubs, the empty `icon.icns`
  placeholder, the `set_master_key` command and the invented per-token cost estimate.

### Security
- No hard-coded fallback master key; master key file is created with `0600` permissions.
- Permissive CORS removed from the proxy; request bodies capped at 64 MiB; API keys are masked in
  command results, debug output and the accounts list.
- Lockfile advisories patched without major upgrades: `openssl`, `rustls`, `rustls-webpki`, `h2`,
  `bytes`, `time`, `tokio`, `anyhow`, `rand`, `crossbeam-epoch`, `event-listener`, `serde_with`.
  Remaining advisories are listed in the README.

## [0.3.0] - 2025-02-03

> **Note:** 0.3.0 was tagged but never built or released. The entries below describe the design
> as it was written at the time, including the standalone CLI that has since been removed; several
> of the features listed did not work (see the fixes under Unreleased).

### Added

#### Core Features
- **Multi-Account Management**: Store and manage multiple OpenAI API accounts with encrypted local storage
- **Usage Monitoring**: Real-time usage tracking with billing integration and limit enforcement
- **Smart Routing**: Intelligent account selection with multiple strategies (least-utilized, round-robin, priority, sticky)
- **Local Proxy Server**: OpenAI-compatible HTTP proxy with streaming support
- **Circuit Breaker**: Automatic failover when accounts hit rate limits or errors
- **Encrypted Storage**: AES-256-GCM encryption with Argon2id key derivation for all credentials
- **Cross-Platform Support**: Native builds for Windows, macOS, and Linux

#### Account Management
- UUID-based account identification
- Support for organization IDs
- Model scoping with wildcard support
- Daily and monthly usage limits
- Priority-based routing preferences
- Enable/disable account states
- Account labeling and metadata

#### Routing Engine
- **Least Utilized Strategy** (default): Selects account with lowest usage ratio
- **Round Robin Strategy**: Even distribution across accounts
- **Priority Strategy**: Tiered fallback system
- **Sticky Strategy**: Session-based routing for prompt caching
- Circuit breaker pattern with automatic recovery
- Real-time account health monitoring

#### Proxy Server
- Axum-based HTTP server
- OpenAI-compatible endpoints:
  - `GET /v1/models` - List available models
  - `POST /v1/chat/completions` - Chat completions with streaming
  - `POST /v1/completions` - Text completions
  - `POST /v1/embeddings` - Embeddings
  - `POST /v1/images/generations` - DALL-E image generation
- Server-sent events (SSE) streaming support
- Authentication middleware
- Request forwarding with account injection
- Error propagation and handling

#### Usage Monitoring
- OpenAI billing API integration
- Subscription and limit fetching
- Token usage tracking
- Exponential backoff polling
- Usage history storage
- Real-time utilization calculations

#### Storage Layer
- SQLite database with WAL mode
- AES-256-GCM encryption at rest
- Argon2id password hashing
- Atomic write operations
- Comprehensive indexing for performance
- In-memory database support for testing

#### User Interface
- Interactive TUI using Ratatui
- Tab-based navigation
- Account list and detail views
- Real-time status updates
- Keyboard shortcuts:
  - `Tab/Arrow Keys`: Navigate tabs
  - `↑/↓`: Navigate account list
  - `a`: Add new account
  - `d`: Delete selected account
  - `e`: Toggle account enabled/disabled
  - `r`: Refresh usage data
  - `q/ESC`: Quit

#### CLI Interface
- Comprehensive command-line interface using Clap
- Subcommands:
  - `proxy` - Start proxy server
  - `add` - Add new account
  - `list` - List all accounts
  - `remove` - Remove account
  - `show` - Show account details
  - `refresh` - Refresh usage data
  - `config` - Manage configuration
  - `tui` - Launch interactive TUI

#### Configuration
- TOML-based configuration files
- Environment variable support
- XDG directory specification compliance
- Hot-reload ready configuration
- Platform-specific default paths

#### Security
- Zero cloud dependency - 100% local processing
- No credential exfiltration
- No sensitive data in logs
- Secure defaults (localhost binding)
- Memory-safe Rust implementation

#### Documentation
- Comprehensive README with quick start
- Architecture documentation
- API documentation
- Build instructions
- OpenCode integration guide

### Technical Implementation

#### Dependencies
- Tokio async runtime
- Axum web framework
- Rusqlite with bundled SQLite
- AES-GCM encryption
- Argon2 password hashing
- Ratatui TUI framework
- Clap CLI parser

#### Performance
- Connection pooling ready
- Prepared statements
- In-memory caching
- Streaming responses
- Efficient header injection
- Low memory footprint (~50MB)

#### Error Handling
- Graceful degradation
- Circuit breaker pattern
- Exponential backoff
- Clear error messages
- Comprehensive logging

### Project Structure
```
codex-manager/
├── Cargo.toml              # Dependencies and build config
├── README.md               # User documentation
├── CHANGELOG.md            # This file
├── src/
│   ├── main.rs            # CLI entry point
│   ├── models/            # Account and usage models
│   ├── storage/           # Encrypted SQLite storage
│   ├── usage/             # OpenAI API client
│   ├── routing/           # Routing engine
│   ├── proxy/             # HTTP proxy server
│   ├── ui/                # TUI interface
│   └── config/            # Configuration management
└── docs/
    ├── ARCHITECTURE.md    # Architecture documentation
    ├── API.md             # API documentation
    └── BUILD.md           # Build instructions
```

### Known Limitations
- Usage API availability depends on OpenAI account type
- TUI input handling could be improved
- Primarily tested on Linux (Windows/macOS testing ongoing)
- Some dependency version conflicts may need resolution

### Future Roadmap
- Web-based GUI option
- Account import/export (encrypted)
- Usage alerts and notifications
- Cost forecasting
- Multi-provider support (Anthropic, Google)
- Docker container
- Prometheus metrics export

[0.3.0]: https://github.com/muhib-karim/codex-manager/releases/tag/v0.3.0
