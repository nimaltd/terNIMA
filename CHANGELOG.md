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
- Serial sessions, each in its own tab. The New serial session dialog (the +
  button, File menu or Ctrl+Shift+N) lists every port with its product,
  maker, USB VID:PID and serial number, updates as devices are plugged in
  and out, and says which ports another tab has open.
- Serial settings: any baud rate, 5 to 8 data bits, no, odd or even parity,
  1 or 2 stop bits, and no, RTS/CTS or XON/XOFF flow control. Port settings
  change on an open port, which is opened again at once.
- A toolbar per tab: Connect and Disconnect, the port and its settings, DTR
  and RTS, Break, CTS, DSR, DCD and RI lights, what Enter sends (CR, LF or
  CR LF) and local echo.
- A port is found again by its USB serial number, so it still connects when
  Windows gives it another COM number. Adapters without a serial number are
  found by their USB socket, their old name, or as the only one of their
  kind. A missing device is waited for, and an unplugged one is connected
  again when it comes back, with a marker line in the terminal for each
  connect, disconnect and reconnect.
- Plain messages when a port cannot be opened: in use by another program,
  open in another tab of ternimal, or, on Linux, missing dialout rights.
- `workspace.toml` keeps the open tabs, each with its port and settings.
  Tabs that were connected when ternimal closed connect again when it starts.
- A status bar with the state of the tab in front, how long it has been
  connected, and the bytes received and sent.
- Ctrl+Tab and Ctrl+Shift+Tab move between tabs; Ctrl+Shift+W closes one.
