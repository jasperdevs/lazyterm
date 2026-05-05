# Lazyterm

Lazyterm is a Rust and GPUI terminal workspace for coding agents.

It is terminal-first: GPUI owns the native window, Alacritty handles terminal state, and the PTY layer runs real local shells and CLI agents.

## Current Shape

- `lazyterm` opens the GPUI window.
- `lazytermctl` controls the running app over the local socket.
- `lazyterm-ui` owns the current app surface.
- `lazyterm-terminal` and `lazyterm-pty` handle terminal state and process execution.
- `lazyterm-sessions` stores session data in SQLite.
- `lazyterm-agents`, `lazyterm-api`, `lazyterm-core`, `lazyterm-git`, and `lazyterm-cli` hold the shared model and command surfaces.
- The navigation direction is vertical tabs for long-lived sessions.

## Development

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo run -p lazyterm-app --bin lazyterm
cargo run -p lazyterm-cli --bin lazytermctl -- status
cargo run -p lazyterm-cli --bin lazytermctl -- send --enter "echo ok"
```

## Windows Local Install

```powershell
cargo build --workspace --release --locked
.\scripts\install-windows.ps1 -SourceDir .\target\release -Launch
```

The installer copies the app into `%LOCALAPPDATA%\Programs\Lazyterm`, creates app shortcuts, and adds `lazytermctl` to the user PATH.

Release assets include `SHA256SUMS`. Windows binaries are Authenticode-signed when the repository signing certificate secrets are configured.

## License

Apache-2.0.
