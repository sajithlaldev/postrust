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

## UI structure
- `src/ui/theme.rs` holds every colour, spacing and font decision (both themes); `widgets.rs` has the badges, pills and painted list rows; `palette.rs` is the Cmd/Ctrl+K palette. The palette's actions live in `src/commands.rs` and run through `App::run_command`.
- Theme, split layout and panel sizes are stored in `Store.ui` (store.json), not eframe persistence.
- `tests/ui_smoke.rs` runs real egui frames headlessly through `ui::draw` (no window) to catch panics and check input handling. Add a case there when adding UI states.

## Conventions
- `ui/` may edit the request/environment rows it is displaying in place. Anything touching the store, tabs, history, disk or network goes through an `App` method (src/app.rs), which is unit-testable via `App::new(None)`.
- Requests run on the tokio runtime owned by `App`; results come back over an mpsc channel polled in `App::logic`.
- `Tab.source = (collection, request)` indices go stale when items are removed; deletion code must detach affected tabs.
- egui 0.37 API: `App::ui`, `Panel::...show(ui, ..)`, `CentralPanel::default().show(ui, ..)`.
- `kv_table` keeps its blank "new row" out of the model until the user types, so viewing never dirties a request.
- Redirects: `http::Http` has a plain and a guarded client; requests with credential-like headers use the guarded one (no cross-origin redirects).
- HTTP tests use `wiremock`; never hit the real network in tests.
