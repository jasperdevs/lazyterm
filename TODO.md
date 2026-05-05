# Lazyterm Worklist

## Working Now

- [x] Repo hygiene and CI polish
- [x] Shared core/API type cleanup
- [x] Session storage hardening
- [x] Terminal and PTY boundary cleanup
- [x] Agent preset/status detection cleanup
- [x] CLI command shape cleanup
- [x] GPUI app/UI polish
- [x] Verification, licensing, and release-risk review
- [x] Real keyboard input path in the GPUI shell surface
- [x] Custom monochrome titlebar with the Lazyterm logo
- [x] Streaming PTY-backed shell sessions in the GPUI surface
- [x] Functional vertical terminal tabs with per-tab shell state
- [x] Windows GUI launch without a separate console window
- [x] Embedded Windows app icon generated from the black-background logo
- [x] Terminal-first monochrome mux surface without dashboard/helper chrome
- [x] Ignored external reference clones for cmux/mux, claude-squad, and seance
- [x] Narrow vertical session rail with active shell state
- [x] Compact in-app view panel for pane mode and terminal font size
- [x] ASCII-only window chrome without emoji/symbol buttons
- [x] Session controls for new, restart, close, and keyboard tab cycling
- [x] Terminal key passthrough for paste, function keys, alt chords, and generic control chords
- [x] Tiled multiplexer view for watching multiple shells at once
- [x] Terminal surface takes focus on app launch
- [x] Alacritty-backed terminal grid for ANSI parsing and cursor movement
- [x] Alacritty terminal writeback replies forwarded to the PTY for Windows ConPTY protocol queries
- [x] Per-cell foreground/background, bold, dim, inverse, underline, and cursor rendering from Alacritty
- [x] Regenerated Windows app icon from the black-background Lazyterm logo
- [x] Tighter monochrome chrome pass: narrow rail, cleaner split mode, and less placeholder copy
- [x] Clean Windows shell launch with PowerShell profiles disabled by default for faster, predictable agent panes
- [x] Command palette for pane/session actions instead of a settings sidebar
- [x] Split command creates a second pane when only one pane exists
- [x] Searchable command palette query with Enter-to-run command execution
- [x] Palette text-input routing that does not leak search text into the active PTY
- [x] Agent pane commands for Codex, Claude, and OpenCode
- [x] Persistent UI settings in `%LOCALAPPDATA%/lazyterm/ui-settings.json`
- [x] Persistent pane manifest in `%LOCALAPPDATA%/lazyterm/sessions.sqlite`
- [x] App socket transport for the CLI/API
- [x] Slop UI cleanup pass: slimmer vertical tabs, SVG logo, selected command row, fewer placeholders
- [x] Agent attention state: needs-input/failed tab marker, statusline notification, focus-attention command
- [x] Mux pane commands for maximize pane and close other panes
- [x] Directional pane navigation with Ctrl+Alt+Arrow and command palette actions
- [x] Persisted terminal density controls for compact/default/roomy layouts
- [x] CLI/API pane controls for close-others and focus-attention
- [x] Persisted pane layout modes for grid, columns, and rows

## Next

- [ ] Deeper terminal-first chrome pass with screenshots against a visual reference
- [ ] Add drag resize handles for pane width/height
- [ ] Add agent pane presets with arguments, working-directory prompts, and health checks
- [ ] Add real split-pane resize handles
- [ ] Add an icon system with a permissive set such as Lucide, Heroicons, Tabler, or Material Symbols
- [ ] Add installer/release workflow after the app can launch and render reliably
- [ ] Add dependency license policy enforcement beyond metadata visibility
- [ ] Add integration coverage for app/UI/API/git boundary crates
- [ ] Prove MSRV with Rust 1.95 in CI instead of stable-only
