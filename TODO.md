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

## Next

- [ ] Add app socket transport for the CLI/API
- [ ] Replace demo sessions with persisted session state
- [ ] Add real split-pane data model and UI interactions
- [ ] Add an icon system with a permissive set such as Lucide, Heroicons, Tabler, or Material Symbols
- [ ] Add installer/release workflow after the app can launch and render reliably
- [ ] Add dependency license policy enforcement beyond metadata visibility
- [ ] Add integration coverage for app/UI/API/git boundary crates
- [ ] Prove MSRV with Rust 1.95 in CI instead of stable-only
