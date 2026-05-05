# Lazyterm Architecture

Lazyterm is a cross-platform coding-agent terminal workspace. It is not a wrapper around tmux and it is not a cmux clone. The repo owns the session model, agent state, worktree flow, transcript history, and local control API.

## Stack

- `lazyterm-app` bootstraps the native GPUI shell.
- `lazyterm-ui` owns the current screen and interaction surface.
- `lazyterm-terminal` handles terminal state and escape-sequence correctness.
- `lazyterm-pty` owns cross-platform shell and agent processes.
- `lazyterm-sessions` provides durable SQLite session state.
- `lazyterm-agents`, `lazyterm-api`, `lazyterm-core`, and `lazyterm-git` carry shared data and command shapes.
- `lazyterm-cli` is a thin request printer for now.
- `gpui` is pinned to a known Zed revision in `Cargo.toml`.
- `alacritty_terminal` and `portable-pty` provide terminal and process primitives.

## Boundaries

- GPUI-specific code stays in `lazyterm-app` and `lazyterm-ui`.
- Terminal emulation and PTY process work stay outside the UI crates.
- Session persistence stays in `lazyterm-sessions`.
- Shared request and status types stay in `lazyterm-core` and `lazyterm-api`.
- Agent detection should stay data-driven and testable from captured output snippets.
- The CLI should stay thin until socket transport exists.
- Competitor code can inform product research, but Lazyterm implementation remains clean-room.

## Product Direction

The default UI uses vertical tabs because coding-agent users often run many long-lived sessions. Tabs should expose agent, branch, status, and the latest attention signal without forcing the user to inspect every pane.
