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
- RTT sessions through a debug probe, using probe-rs: CMSIS-DAP, J-Link,
  ST-Link and the other probes it supports, with no vendor software. The +
  button and File > New RTT session (Ctrl+Shift+R) open a dialog with a live
  probe list, a chip search over probe-rs's thousands of chips, and the
  firmware's ELF file to find the control block from its `_SEGGER_RTT`
  symbol. Without one, RAM is searched; a fixed address works too.
- RTT options: up and down channel, SWD speed, and connect under reset for
  firmware that sleeps or turns the debug pins off.
- An RTT session attaches without halting or resetting the chip. It waits
  for firmware that sets up RTT late, finds the control block again after a
  reset or new firmware, and attaches again when the probe comes back.
- The RTT toolbar: the chip, the channel shown, and a Reset target button.
- RTT tabs are kept in `workspace.toml` with the serial ones, and connect
  again at start.
- SWO sessions through a debug probe: what the firmware writes to the ITM
  stimulus ports, read from the trace pin. The + button and File > New SWO
  session open the same probe dialog, with the core clock, the SWO baud rate
  (checked to divide the clock exactly), and one stimulus port or all. The
  toolbar has a port picker and Reset target; overflows of the chip's trace
  FIFO are noted in the terminal.
- On STM32H5 chips ternimal turns the trace pin on itself, at DBGMCU_CR
  0x44024004, as probe-rs looks for it where the U5 and L5 have it.
- A shared probe module for RTT and SWO: the probe list, the chip search and
  attaching.
- Log files. Each tab turns its log on or off with the Log button in the
  toolbar, File > Log this tab to a file, or Ctrl+Shift+L. A text log has
  the colors and other escape codes removed and the date and time at the
  start of every line; a raw log keeps every byte. Connects, disconnects
  and reconnects get a marker line. Every log starts a new file named after
  the tab and the time, and the status bar shows it; a click shows it in
  its folder. A failed write turns the log off and says why.
- A Settings window (File > Settings, Ctrl+,) with the theme, the text size
  and the logging options: log new tabs, the folder ("ternimal logs" in
  Documents unless set), the format, and the date and time on each line.
- File > Save output as (Ctrl+Shift+S) saves what the terminal shows,
  scrollback and all, as text. File > Open log folder shows the logs.
- Example programs in `crates/ternimal-core/examples`: `probes` lists probes
  and reads an STM32's ID register, `rtt` and `swo` run a session from the
  command line, and `flash` writes an ELF file to a chip.
