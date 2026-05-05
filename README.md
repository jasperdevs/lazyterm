# Lazyterm

Lazyterm is a GPUI terminal workspace for local shells and coding agents.

It uses real PTYs, Alacritty's terminal parser, vertical session tabs, split panes, and a local control CLI. The app is built in Rust and tested on Windows, macOS, and Linux.

## Run

```sh
cargo run -p lazyterm-app --bin lazyterm
```

In another shell:

```sh
cargo run -p lazyterm-cli --bin lazytermctl -- status
cargo run -p lazyterm-cli --bin lazytermctl -- split
cargo run -p lazyterm-cli --bin lazytermctl -- send --enter "cargo test"
```

## Windows Install

```powershell
cargo build --workspace --release --locked
.\scripts\install-windows.ps1 -SourceDir .\target\release -Launch
```

The installer copies Lazyterm to `%LOCALAPPDATA%\Programs\Lazyterm`, creates Start Menu and desktop shortcuts, and adds `lazytermctl` to the user PATH.

## Controls

| Action | Shortcut or command |
| --- | --- |
| New shell | `Ctrl+Shift+T` or `lazytermctl new` |
| Command palette | `Ctrl+Shift+P`, `Ctrl+Shift+K`, or `Ctrl+Shift+,` |
| Split workspace | `Ctrl+Shift+B` or `lazytermctl split` |
| Maximize active pane | `Ctrl+Shift+Enter` or `lazytermctl maximize` |
| Switch tabs | `Ctrl+Shift+1` through `Ctrl+Shift+9`, or `Ctrl+Tab` |
| Focus pane direction | `Ctrl+Alt+Arrow` |
| Launch Codex task | `lazytermctl run --cwd . --task "fix the parser"` |

Run `lazytermctl help` for the full control surface.

## Release

Tagged releases build Windows, macOS, and Linux archives. Release assets include `SHA256SUMS`; Windows binaries are Authenticode-signed when the repository signing certificate secrets are configured.

## Development

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
```

## License

Apache-2.0.
