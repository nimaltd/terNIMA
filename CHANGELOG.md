# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0] - 2026-10-25

The first release, for Windows, macOS and Linux. The apps are not signed
yet; the README says how to open them.

### Changed

- The project is called ternima now. It started as ternimal, a name another
  project already had.
- The welcome logo reads terNIMA: a small "ter" in teal and "NIMA" in big
  orange block letters, each with its shadow.
- The app shows its name as terNIMA: the window title, the About box, the
  menus, the status bar and its messages. It installs as terNIMA on
  Windows and macOS; on Linux, where package and program names are
  lowercase, it stays ternima.

### Added

- The project plan in `docs/PLAN.md`: what ternima does, what it is built
  with, and the order it is built in.
- README, Apache 2.0 licence and NOTICE files.
- The app skeleton: a Rust workspace with `ternima-core` and the Tauri 2 app,
  and a Svelte UI with a tab bar, an xterm.js terminal and a status bar.
- A welcome text in the terminal that shows the logo, the version, and every
  kind of color and style the terminal draws: 16, 256 and 24-bit color, bold,
  dim, italic, underline, inverse and strikethrough. The logo is TERNIMA in
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
  open in another tab of ternima, or, on Linux, missing dialout rights.
- `workspace.toml` keeps the open tabs, each with its port and settings.
  Tabs that were connected when ternima closed connect again when it starts.
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
- On STM32H5 chips ternima turns the trace pin on itself, at DBGMCU_CR
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
  and the logging options: log new tabs, the folder ("ternima logs" in
  Documents unless set), the format, and the date and time on each line.
- File > Save output as (Ctrl+Shift+S) saves what the terminal shows,
  scrollback and all, as text. File > Open log folder shows the logs.
- The window opens where it was and at the size it had, maximized if it
  was, and goes back to its earlier size when un-maximized. If the screen it
  was on has been unplugged, it opens in the middle of the main screen. It
  starts hidden until it is in place, so it never jumps.
- Example programs in `crates/ternima-core/examples`: `probes` lists probes
  and reads an STM32's ID register, `rtt` and `swo` run a session from the
  command line, `flash` writes an ELF file to a chip, and `detect` finds out
  which chip is on the probe.
- The probe dialog finds out the chip by itself: the Detect button, or at
  once when a probe is picked and the chip is still empty. STM32 chips are
  told apart by their DBGMCU ID register, which probe-rs does not read, and
  the dialog lists the chips of the family when the package is not known
  (STM32H503CB, EB, KB or RB). Other chips use probe-rs's own detection.
  It does not halt or reset the chip.
- Find in the terminal output: Edit > Find (Ctrl+Shift+F) opens a search bar
  over the terminal with match case, whole word and regex, the number of
  matches, and every match marked. Enter and F3 go to the next match;
  Shift+Enter and Shift+F3 to the one before.
- Split view: View > Split right (Ctrl+Shift+E) or Split down (Ctrl+Shift+D)
  shows the next tab next to the one in front, each with its own toolbar.
  Drag the line between them to share the room, or double-click it for
  halves. A click or F6 gives the keyboard to the other pane, and a tab
  picked in the tab bar takes the place of the pane that has the keyboard.
  The split is kept in `workspace.toml`.
- Startup settings: whether ternima reopens the tabs from last time,
  connects the ones that were connected, and opens the window where it was.
  All three are on unless turned off in Settings.
- TCP sessions (File > New TCP session, Ctrl+Shift+T, or the + button), to
  a host and port: plain bytes both ways for a device or a serial bridge on
  the network, or Telnet, whose option talk is answered and kept out of the
  terminal. A session waits for the other end and connects again when it
  comes back, with a marker line, the same as a serial port.
- RTT through J-Link or OpenOCD, from the + button: ternima reads RTT from
  the server J-Link's software (port 19021) or OpenOCD (`rtt server
  start`) opens, so a debugger can keep the probe. J-Link is asked for
  another RTT channel when one is chosen. When a debugger has the probe,
  the RTT dialog's message points here.
- `ternima-core` has a `net` module for these sessions, tested against
  servers on the same machine.
- A send box under each terminal (View > Send box): type a whole line and
  send it at once, as text or as hex bytes such as `AA 55 01`. Text can
  hold `\r`, `\n`, `\t`, `\e`, `\0` and `\xHH`. Enter sends the tab's line
  ending after it. ↑ and ↓ bring back what was sent; each tab keeps its
  last 100 lines in `workspace.toml`.
- Macros (Tools > Macros): named commands, text or hex, shown as buttons
  under the send box and sent with Alt+1 to Alt+9 (Cmd+1 to Cmd+9 on
  macOS). They are kept in `settings.toml`.
- Tools > Send file sends a file through the tab, in pieces paced for the
  link (the baud rate for a serial port), with its progress and a Stop
  button in the status bar.
- A hex view (View > Hex, Ctrl+Shift+H, or the Hex button): every byte
  sent (TX) and received (RX), with offsets and the bytes as characters.
  A line holds 8, 16 or 32 bytes, and a new one starts when the direction
  changes or after a chosen byte such as `0A`. The terminal keeps
  everything while the hex view is shown.
- A Lines view (View > Lines, Ctrl+Shift+J, or Terminal · Hex · Lines in
  the toolbar): every received line in a list, with Zephyr's log lines read
  into time, level and module. Level chips and a module list with counts
  show or hide lines, a search marks matches or keeps only them, highlight
  rules (Tools > Highlight rules) color the lines that match, and bookmarks
  (Ctrl+F2, then F2 and Shift+F2) mark lines to come back to. The list
  keeps up with the newest line until scrolled up, where a "Latest ↓"
  button takes it back. It holds 100,000 lines, its counts keep their
  width as they grow, and
  Copy, Select all, Find and Save output as work on it. Its filters are
  kept per tab in `workspace.toml`, and the rules in `settings.toml`.
- A command palette (View > Command palette, Ctrl+Shift+P or Cmd+Shift+P):
  a few letters find any menu command that can be used now, a tab to go
  to, or a macro to send, such as "sf" for Send file. The matched letters
  are marked, a check shows what is on, and the commands used lately come
  first when nothing is typed.
- A Modbus view (View > Modbus, Ctrl+Shift+M, or Modbus in the toolbar):
  a Modbus RTU, ASCII or TCP tester on the tab's own connection, serial or
  TCP. As a master, a list of requests (functions 1 to 6, 15 and 16, each
  with its unit, address and count or values to write) is sent one by one
  or polled every 100 ms to 5 s; each shows what came back, in its time
  and how long it took, as unsigned, signed, hex or 32 bit floats either
  word first, or the exception and its name, or that no answer came in
  the timeout. Scan units asks 1 to 247 and lists those that answer. The
  Plot view's Read: Modbus plots what is polled, named hr0, ir5, c3. As a
  slave, it answers a master for its unit from tables of holding and
  input registers, coils and discrete inputs that can be edited, and that
  a master's writes change, with exceptions for what it cannot do. Both
  keep a log of every message, in words and in hex, and counts of what
  was sent, answered, timed out and wrong.
- Modbus RTU and Modbus ASCII in the Frames view: requests and answers
  read off a bus and checked, each in words, with a read's values named by
  their address for the plot, and exceptions marked.
- A Frames view (View > Frames, Ctrl+Shift+B, or Frames in the toolbar):
  the data split into frames, each checked and its fields read. Frames
  can be of your own layout (start bytes, a length of 1 or 2 bytes or a
  fixed size, fields from u8 to f64, either byte order, with a scale, and
  a sum, XOR, CRC-8, CRC-16 Modbus, CCITT or XMODEM, or CRC-32 check at
  the end), COBS, SLIP, or NMEA sentences from a GPS receiver, read into
  lat, lon, alt, sats, speed and course. Tools > Frame layout shows how
  many frames a layout finds in what came lately while it is changed, and
  a new layout reads that data again. The list marks each frame right or
  wrong, with why; the frame picked shows its bytes, colored by start,
  length, fields and check, and its fields. CSV... saves the frames, and
  the Plot view's Read: Frames plots their fields, as XY too.
- Record and replay. Rec in the toolbar, or File > Record this tab,
  records every byte a tab receives and sends, with when, to a `.trec`
  file in the log folder, named after the tab and the time. It is a text
  file that can be read: a line for each piece, with its time, `<` or `>`,
  and the bytes in quotes, escaped where they are not text. File > Play a
  recording (Ctrl+Shift+O, or "A recording" under the + of the tab bar)
  plays one in a tab of its own, with its timing: Play and Pause, Restart,
  0.25× to 100×, and a slider to go to any point. The terminal, hex,
  Lines and Plot views show it as they showed the session, sent bytes
  included, and lines keep the time they were recorded at, so the plot
  shows the real timing at any speed. A replay tab comes back paused the
  next time terNIMA opens.
- A Plot view (View > Plot, Ctrl+Shift+G, or Plot in the toolbar): the
  numbers in the received lines, drawn over time. It reads `temp=21.5
  hum=40`, the Arduino Serial Plotter's `temp:21.5,hum:40`, plain
  `21.5,40` or `21.5 40`, and JSON lines such as `{"temp": 21.5}`, also
  inside Zephyr log lines; or, for any other line, a regex with a capture
  group for each value. It follows the newest values over 5 s to 5 min, or
  shows them all. Drag or the wheel zooms, Shift+wheel moves, Space pauses
  and a double-click follows again. Clicks place the cursors A and B,
  which show the time between them as a period and a frequency, and each
  series' value at A and B. A table under the chart shows each series'
  value under the mouse, and its min, max and mean between A and B or in
  view; its boxes show or hide a series. CSV... saves every value with its
  time, and PNG... the chart as shown, with the series' names. Up to 24
  series, in colors that color blind eyes tell apart too. It keeps what the
  Lines view keeps, so it can show what came before it was opened; Clear
  starts it again. How it reads and what it hides are kept per tab in
  `workspace.toml`.
- The Plot view draws four charts, picked next to Read: Time, XY,
  Histogram and FFT. The others work on the time the time chart shows, or
  between A and B, and keep up with the values a few times a second.
  - XY: one series along x, the others against it as points.
  - Histogram: the shown series' values counted in shared bins, Auto or
    10 to 100; whole numbers get a bin each, centered on them.
  - FFT: each shown series' amplitude spectrum, in dB or as amplitudes, a
    sine of amplitude 1 being 0 dB, with a Hann, Hamming, Blackman or no
    window. Lines are taken as evenly spaced at the measured rate, or at
    one typed in. The table has each series' peak, found between bins.
  - CSV... saves what the chart shows: values, bins or the spectrum.
- A scope's trigger on the time chart: when a series crosses a level,
  rising, falling or either way, the time around the crossing is shown
  with the crossing in the middle, marked T. Single holds the first
  crossing until Arm.
- Spans of 100 ms to 2 s, besides 5 s to 5 min and all, and a standard
  deviation for each series in the table.
- Received lines are timed as their bytes are read, so the plot and the
  Lines view have the time they came, not when the window drew them.
  Lines read at once are spread over the time since the read before, up
  to 200 ms, so a fast stream draws as a curve rather than in steps.
- UI unit tests, run by `npm test` and in CI.
- "New line after" in the toolbar: data that comes after a pause of 50 ms
  to 2 s starts on a new line, in the terminal and in the hex view, for
  devices that send messages without line endings. Not after what was just
  typed or sent, so a shell's echo stays on its line. Only the view
  changes; logs keep the bytes as they came.

### Removed

- Flash programming is no longer planned, and its menu item is gone.
- The dashboard is no longer planned: the Plot view's table shows the
  numbers, and macros send commands.
- The HardFault decoder and semihosting are no longer planned: they belong
  in a debugger more than in a terminal.

### Fixed

- The toolbar no longer hides some of its views when the window is
  narrow, as it did with Lines on a serial tab: what does not fit goes to
  a second row on the right.
- A serial port no longer seems unplugged when a long write takes more
  than its short timeout, as on Windows: a long paste or a file went out
  as one piece and the port was opened again. A port that takes no bytes
  at all for 2 s, as when flow control holds it, drops the rest and stays
  open.

[Unreleased]: https://github.com/nimaltd/ternima/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/nimaltd/ternima/releases/tag/v1.0.0
