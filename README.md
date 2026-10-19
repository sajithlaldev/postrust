<p align="center">
  <img src="logo.png" alt="postrust logo" width="160">
</p>

# PostRust

A small Postman-style API client written in Rust with [egui](https://github.com/emilk/egui).

## Run

Requires Rust 1.88 or newer.

```bash
cargo run --release
```

## macOS app bundle

The window and Dock icon come from `logo.png` (resized copies live in `assets/`). To build a `.app` with the icon:

```bash
cargo install cargo-bundle
cargo bundle --release      # target/release/bundle/osx/postrust.app
```

The bundle identifier in `Cargo.toml` (`dev.postrust.app`) is a placeholder; change it to your own.

## Features

- Request builder: method, URL, query params, headers, body (raw / urlencoded form), auth (Bearer, Basic, API key header)
- Response viewer: status, time, size, syntax-highlighted pretty-printed JSON (with a Pretty / Raw toggle), headers, copy / save body
- Request body: JSON syntax highlighting and a **Beautify JSON** button (keeps `{{variables}}` as written)
- Postman-style layout: tabs with method badges and an unsaved-changes dot, a searchable sidebar, and the response below or beside the request (panel sizes are remembered)
- Light, dark or system theme, coloured method badges and status pills
- Command palette (`Cmd/Ctrl + K`) and **Copy as cURL**
- Collections and request history, persisted as JSON
- Environments with `{{variable}}` substitution (URL, params, headers, body, auth); undefined variables are highlighted
- Multiple request tabs, cancellable requests, configurable timeout
- Import / export collections as JSON (a marked `postrust-collections` file; other JSON is rejected, and export only overwrites an earlier export)

Shortcuts: `⌘/Ctrl + K` command palette, `+ Enter` send, `+ S` save, `+ T` new tab, `+ W` close tab.

## Data

Stored in `<config dir>/postrust/store.json` (macOS: `~/Library/Application Support/postrust`).
Tokens and passwords are saved **unencrypted** (file mode 0600 on Unix). A corrupt store is moved to `store.json.bak`; an unreadable one is never overwritten.

## Behaviour notes

- A URL without a scheme gets `https://` (`http://` for localhost / 127.0.0.1).
- Beautify JSON and the Pretty view parse the body, so duplicate object keys collapse to the last value and string escapes are normalised (`\\u00e9` becomes `é`); use Raw (response) or don't beautify (request) if you need repeated keys.
- Redirects are followed (max 10). Cookies are not persisted.
- History does not store Auth-tab credentials, but it does keep the URL and any headers you typed yourself (tokens in them are saved in plaintext).
- Sending is refused while the URL, headers or Auth use an undefined `{{variable}}`; undefined ones in params or the body only show a warning.
- Requests with an API-key auth or a credential-looking custom header (name contains key/token/secret/auth/session/password) don't follow redirects to another origin; you see the 3xx instead. Authorization and Cookie are stripped by reqwest on such redirects.
- The Auth tab replaces a manually typed header of the same name.
- Downloads stop at 20 MB; the viewer shows at most 200 KB of text.
- "Save body" writes to your Downloads folder.

## Releases

Pull requests into `main` run fmt, clippy and the tests. Pushing to `main` (for example by merging a PR) runs `.github/workflows/release.yml`. It tests the code, then, **if the `version` in `Cargo.toml` has no `vX.Y.Z` tag yet**, builds the app and publishes a GitHub Release with generated notes and these files:

| Platform | Asset |
|---|---|
| macOS Apple Silicon | `postrust-<version>-macos-arm64.dmg` |
| macOS Intel | `postrust-<version>-macos-x64.dmg` |
| Linux x64 | `postrust-<version>-linux-x64.tar.gz` |
| Windows x64 | `postrust-<version>-windows-x64.zip` |

plus `SHA256SUMS.txt`. **To cut a release, bump `version` in `Cargo.toml` (and commit the updated `Cargo.lock`) in your PR.** Merges that don't bump it publish nothing.

### Verifying downloads

Every release file is signed with [cosign / Sigstore](https://docs.sigstore.dev/) (keyless, tied to this repository's release workflow) and has a GitHub build attestation. This proves the file was built by this repo's workflow and wasn't altered. It does not remove the OS warnings below (that needs paid Apple/Windows certificates).

```bash
# 1. Checksums
shasum -a 256 -c SHA256SUMS.txt --ignore-missing

# 2. GitHub build attestation
gh attestation verify postrust-<version>-macos-arm64.dmg --repo sajithlaldev/postrust

# 3. cosign signature (download the matching .cosign.bundle next to the file)
cosign verify-blob postrust-<version>-macos-arm64.dmg \
  --bundle postrust-<version>-macos-arm64.dmg.cosign.bundle \
  --certificate-identity-regexp '^https://github.com/sajithlaldev/postrust/\.github/workflows/release\.yml@refs/heads/main$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

The signing job runs after the release is published; if it fails the release stays available but unsigned until the job is re-run.

The apps are **not Apple-notarized or Windows-signed**. On macOS, right-click the app and choose Open the first time (or run `xattr -dr com.apple.quarantine /Applications/postrust.app`). On Windows, choose "More info", then "Run anyway".

## Roadmap

Plans, not promises. Rough order within each group; open an issue if something matters to you.

**Next**
- Import and export in the Postman v2.1 collection format
- Native file dialogs for import, export and "Save body" (today they use a typed path / the Downloads folder)
- `multipart/form-data` and binary / file bodies
- Cookie jar, and proxy settings in the app
- Search inside responses and a JSON path filter
- Encrypt saved secrets (OS keychain) instead of plain-text `store.json`
- Apple notarization and Windows code signing, so the first-launch warnings go away (needs paid certificates)

**Later**
- Test scripts and assertions, plus a collection runner
- GraphQL and WebSocket requests
- Copy a request as code for other languages (fetch, Python requests, ...)
- Per-request saved response examples and a diff view
- In-app update check
- A **web build** (see below)

**Web build (exploratory)**

The UI is egui, which can run in the browser via WebAssembly, so a hosted or offline-capable web version is technically possible. It is not a drop-in compile, and it has one limit that code can't remove:

- Browsers only let a page call another origin if that server sends CORS headers, and they forbid setting some headers (`Host`, `Cookie`, `Origin`, ...) and controlling redirects. A web build would fail against many APIs unless requests go through a small proxy you run, or a browser extension. The desktop app has no such limit, which is why it stays the primary target.
- Code changes needed: replace the multi-threaded tokio runtime (it doesn't compile for `wasm32`; first error seen when trying), swap the file-based storage and the `directories` crate for `localStorage` / IndexedDB, replace file export / import / "Save body" with browser downloads and uploads, use a wasm-safe clock instead of `std::time::Instant`, and gate the native-only `reqwest` features (rustls, system proxy, custom redirect policy).
- Deployment would add a `trunk` build and a GitHub Pages step to the release workflow.

## Develop

```bash
cargo test            # unit + integration tests (HTTP tests use a local mock server)
cargo clippy --all-targets
```

Layout: `src/model.rs` (data), `vars.rs` (`{{var}}`), `http.rs` (prepare + send), `storage.rs`, `app.rs` (state and actions), `ui/` (egui views).
