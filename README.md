<p align="center">
  <img src="logo.png" alt="postrust logo" width="160">
</p>

# postrust

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
- Response viewer: status, time, size, pretty-printed JSON, headers, copy / save body
- Collections and request history, persisted as JSON
- Environments with `{{variable}}` substitution (URL, params, headers, body, auth); undefined variables are highlighted
- Multiple request tabs, cancellable requests, configurable timeout
- Import / export collections as JSON (a marked `postrust-collections` file; other JSON is rejected, and export only overwrites an earlier export)

Shortcuts: `⌘/Ctrl + Enter` send, `+ S` save, `+ T` new tab, `+ W` close tab.

## Data

Stored in `<config dir>/postrust/store.json` (macOS: `~/Library/Application Support/postrust`).
Tokens and passwords are saved **unencrypted** (file mode 0600 on Unix). A corrupt store is moved to `store.json.bak`; an unreadable one is never overwritten.

## Behaviour notes

- A URL without a scheme gets `https://` (`http://` for localhost / 127.0.0.1).
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

The builds are **unsigned**. On macOS, right-click the app and choose Open the first time (or run `xattr -dr com.apple.quarantine /Applications/postrust.app`). On Windows, choose "More info", then "Run anyway".

## Develop

```bash
cargo test            # unit + integration tests (HTTP tests use a local mock server)
cargo clippy --all-targets
```

Layout: `src/model.rs` (data), `vars.rs` (`{{var}}`), `http.rs` (prepare + send), `storage.rs`, `app.rs` (state and actions), `ui/` (egui views).
