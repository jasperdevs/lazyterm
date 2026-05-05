# Lazyterm

Lazyterm is a Rust and GPUI terminal workspace for coding agents.

This repository is still a scaffold. The current focus is the session model, agent state, transcript history, worktree flow, and local control API.

## Current Shape

- `lazyterm-app` opens the GPUI window.
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
cargo run -p lazyterm-app
```

## License

Apache-2.0.
