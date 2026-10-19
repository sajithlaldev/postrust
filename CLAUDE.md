# postrust

Rust + egui (eframe 0.37) Postman-style API client. See README.md for features.

## Assets
- `logo.png` is the source logo. `assets/icon-256.png` (window icon), `assets/icon-64.png` (top bar) and `assets/AppIcon.icns` (macOS bundle) are generated from it with `sips`/`iconutil`; regenerate them if the logo changes.

## Releases
- `.github/workflows/release.yml` runs fmt/clippy/test on PRs and pushes to `main`; on push to `main` it then builds and publishes a GitHub Release only if `v<Cargo.toml version>` has no tag yet. Bump `version` (and `Cargo.lock`) to release.
- A `sign` job runs after publishing: GitHub build attestations plus cosign keyless `.cosign.bundle` bundles for every asset. OS-level signing (Apple Developer ID + notarization, Windows Authenticode) is not set up; it needs paid certificates and repo secrets.
- macOS `.app`/`.dmg` is assembled by `scripts/make-macos-app.sh` (ad-hoc signed, not notarized); the macOS Intel build is cross-compiled on the ARM runner.
- Validate workflow edits with `actionlint`; the CI test job uses `clippy -D warnings` and `--locked`.

## Commands
- `cargo run` / `cargo test` / `cargo clippy --all-targets`

## Web (WebAssembly) build
- Everything that differs between desktop and browser lives in `src/platform/` (`native.rs`, `web.rs`): background tasks (`Executor`/`Task`, `run_blocking`), the browser `localStorage` store backend, downloads, default paths. Cargo splits native-only crates (tokio runtime, `directories`, rustls, system proxy, wgpu) from web ones with `[target.'cfg(...)'.dependencies]`; the web uses eframe's `glow` backend.
- `storage.rs` owns the load/save/corruption policy and talks to a `Backend` trait (file or `localStorage`); test policy changes with the in-memory backend there.
- `App.web_mode` switches the web-only UI (paste-to-import box, hints, header warning); it defaults to `cfg!(target_arch = "wasm32")` but is a plain field so tests can exercise those layouts natively.
- Don't use `std::time::Instant` (panics in browsers; use `web_time`), threads, or `std::fs` outside `cfg(not(target_arch = "wasm32"))`. Check both targets: `cargo check --lib --bin postrust --target wasm32-unknown-unknown`.
- Build/serve with Trunk (`trunk serve --release`); CI builds it on every run and deploys to GitHub Pages from `main`. Browser behaviour (CORS, dropped headers, storage) can only be proven in a real browser: drive headless Chrome over the DevTools protocol, never trust compile success alone.

## cURL import
- `src/curl.rs` is a pure parser (shell-style tokenizer plus option table); it never executes anything or reads files. `App::apply_import` applies a parsed command with an Undo (`App::import_curl` is the text-in convenience wrapper, used by tests). The URL field's paste interception parses once and hands the result over; it lives in `ui/request.rs` (`url_field_id` is shared with the tests).
- `tests/curl_roundtrip.rs` checks that `http::to_curl` output parses back to the same request: keep the two in sync when changing either.

## UI structure
- `src/ui/theme.rs` holds every colour, spacing and font decision (both themes); `widgets.rs` has the badges, pills and painted list rows; `palette.rs` is the Cmd/Ctrl+K palette. The palette's actions live in `src/commands.rs` and run through `App::run_command`.
- Theme, split layout and panel sizes are stored in `Store.ui` (store.json), not eframe persistence.
- `tests/ui_smoke.rs` runs real egui frames headlessly through `ui::draw` (no window) to catch panics and check input handling. Add a case there when adding UI states.

## Conventions
- `ui/` may edit the request/environment rows it is displaying in place. Anything touching the store, tabs, history, disk or network goes through an `App` method (src/app.rs), which is unit-testable via `App::new(None)`.
- Requests run on `platform::Executor` (tokio natively, the browser event loop on the web; never use tokio directly in shared code); results come back over an mpsc channel polled in `App::logic`.
- `Tab.source = (collection, request)` indices go stale when items are removed; deletion code must detach affected tabs.
- egui 0.37 API: `App::ui`, `Panel::...show(ui, ..)`, `CentralPanel::default().show(ui, ..)`.
- `kv_table` keeps its blank "new row" out of the model until the user types, so viewing never dirties a request.
- Redirects (desktop): `http::Http` has a plain and a guarded client; requests with credential-like headers use the guarded one (no cross-origin redirects). In the browser `fetch` follows redirects itself, so both clients are the same and the web UI warns about credential headers instead.
- HTTP tests use `wiremock`; never hit the real network in tests.
