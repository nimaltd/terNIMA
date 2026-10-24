# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The project plan in `docs/PLAN.md`: what ternimal does, what it is built
  with, and the order it is built in.
- README, Apache 2.0 licence and NOTICE files.
- The app skeleton: a Rust workspace with `ternimal-core` and the Tauri 2 app,
  and a Svelte UI with a tab bar, an xterm.js terminal and a status bar.
- A welcome text in the terminal that shows the logo, the version, and every
  kind of color and style the terminal draws: 16, 256 and 24-bit color, bold,
  dim, italic, underline, inverse and strikethrough. The logo is TERNIMAL in
  block letters with a shadow edge and NIMA in orange, in colors picked for
  the dark or the light theme.
- JetBrains Mono and Inter fonts shipped with the app, and dark and light
  themes that follow the OS.
- CI on Windows, macOS and Linux: UI type check and build, `cargo fmt`,
  `cargo clippy` and the tests.
- A release workflow that builds the installers for every OS from a tag.
- A menu bar inside the window, the same on every OS: File, Edit, View,
  Connection, Tools and Help. Alt+F, Alt+E and so on open it from the
  keyboard on Windows and Linux. Items that later phases bring are listed
  already, greyed out with their phase.
- Copy, paste, select all and clear screen, on Ctrl+Shift+C, V, A and K
  (Cmd+C, V, A and K on macOS), so Ctrl+C still reaches the device.
- Bigger and smaller text (Ctrl+=, Ctrl+-, Ctrl+0) and a theme choice:
  follow the OS, dark or light. The window frame follows the theme too.
- `settings.toml` in the OS's config folder keeps the theme and text size.
  It is saved on every change, through a temporary file so it is never left
  half written. A broken file is kept as `settings.bad.toml` and the
  defaults are used.
- An About box with the version and licence.
