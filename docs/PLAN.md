# ternima plan

ternima is a desktop terminal for embedded work: serial ports, SEGGER RTT through
any common debug probe, Zephyr's shell and logs, and the tools around them, in one
app that looks and works the same on Windows, Linux and macOS.

The name is "terminal" with my name, Nima, in it: ter-NIMA.

This file is the plan: what we build, what we build it with, and in which order. It
will change as we learn. The checklists at the bottom show where we are.

---

## Decisions

| Topic | Decision |
|---|---|
| Kind of app | Desktop GUI app with its own window |
| Platforms | Windows 10/11, Linux x86_64, macOS 11+ (Apple Silicon and Intel). Linux ARM64 later |
| Look | The same UI on all three. Only the window frame comes from the OS |
| Menus | Our own menu bar inside the window (File, Edit, View, Connection, Tools, Help), the same on every OS |
| Language | Rust for everything that touches hardware, TypeScript for the UI |
| App framework | Tauri 2 |
| Terminal engine | xterm.js |
| Charts | uPlot |
| License | Apache 2.0, like the other NimaLTD projects |
| Config format | TOML, in the OS's normal config folder |

---

## Why Rust and Tauri

- **probe-rs** is a Rust library that talks to CMSIS-DAP, J-Link and ST-Link probes
  directly and already does RTT, flashing and memory access. No other language has
  anything this complete. With it, RTT over a DAPLink or a J-Link is a library call,
  not a project of its own.
- **Tauri 2** wraps a Rust program in the OS's own web view. The app is about 10 MB,
  where an Electron app is over 100 MB, and it starts fast.
- **xterm.js** is the terminal engine inside VS Code. It handles VT100, VT220 and
  xterm escape codes, 256 colors and 24-bit color, and it is fast.
- **The same look everywhere.** The UI is our own HTML and CSS, drawn by the OS's web
  view (WebView2 on Windows, WebKit on macOS and Linux), so it looks the same on all
  three. We ship our own fonts (JetBrains Mono for the terminal, Inter for the UI,
  both under the SIL Open Font License) so text looks the same too.

Other options we looked at:

| Option | Why not |
|---|---|
| C++ and Qt | Heavy to build, LGPL licensing questions, no good cross platform VT100 widget, no probe library |
| Python | Quickest prototype, but too slow at multi-Mbaud or fast RTT, and hard to ship as one app |
| Electron | Works, but large and uses a lot of RAM |
| C# and Avalonia | Good UI, but no probe or RTT library |
| Rust with egui | One language, but we would have to build the terminal emulator ourselves |

---

## The main window

```
+-------------------------------------------------------------------------+
| File  Edit  View  Connection  Tools  Help                               |  menu bar
+-------------------------------------------------------------------------+
| [+]  Nucleo UART *  |  nRF52 RTT *  |  ESP32 (waiting)  |          [gear] |  tabs
+-------------------------------------------------------------------------+
| COM5  115200 8N1  | DTR [x] RTS [ ] | CTS DSR DCD RI | Terminal Log Hex Plot | toolbar
+-------------------------------------------------------------------------+
|                                                                         |
|   the view: terminal, log, hex or plot                                  |
|   (split into two or more panes if you like)                            |
|                                                                         |
+-------------------------------------------------------------------------+
| > send box                [CRLF v] [Send]    macros: [reset] [help] [ver] |
+-------------------------------------------------------------------------+
| Connected 00:12:31 | RX 1.2 MB | TX 3 KB | Logging to nucleo-2026-10-24.log |  status bar
+-------------------------------------------------------------------------+
```

- The menu bar is drawn by ternima, so it looks the same on every OS and follows
  the theme. Alt+F, Alt+E and so on open the menus on Windows and Linux. Items that
  a later phase brings are listed already, greyed out with their phase number.
- In the terminal view you type straight into the device, as in PuTTY.
- The send box is there in every view, for whole lines, hex bytes and history.
- A command palette (Ctrl+Shift+P, Cmd+Shift+P on macOS) reaches every action by name.

---

## Connections

### Serial

- Any baud rate, including custom ones and 1M, 2M, 3M and higher
- Data bits 5, 6, 7 or 8
- Parity none, odd or even. Mark and space parity, and 1.5 stop bits, later: the
  `serialport` library does not offer them yet
- Stop bits 1 or 2
- Flow control none, RTS/CTS or XON/XOFF
- DTR and RTS buttons, send break, and live CTS, DSR, DCD and RI lights
- The port list shows each USB port's name, maker, VID:PID and serial number, and
  updates by itself when you plug or unplug something

### RTT through a debug probe (probe-rs)

- Probes: CMSIS-DAP v1 and v2 (DAPLink, Pico probe and others), J-Link, ST-Link v2
  and v3, ESP USB-JTAG, and the others probe-rs supports
- Chip: picked from probe-rs's list of chips, and remembered
- Control block: found from the `_SEGGER_RTT` symbol in an ELF file (fast and
  exact), or by scanning RAM (no file needed)
- Channels: every up channel can be shown, each in its own pane if you like. The
  down channel takes what you type
- Attaches without halting or resetting the target by default. Reset on attach and
  attach under reset are options
- SWD speed setting
- Only one program can use a probe at a time. While your IDE debugs, use RTT over TCP

### RTT over TCP

- J-Link: while J-Link GDB Server, J-Link Commander, Ozone or your IDE has a J-Link
  session open, SEGGER serves RTT on `localhost:19021`. ternima connects there, so
  you see RTT while you debug
- OpenOCD: after `rtt server start <port> <channel>`, ternima connects to that port

### J-Link through SEGGER's own library (later)

- ternima loads `JLinkARM.dll`, `libjlinkarm.so` or `libjlinkarm.dylib` from your
  J-Link install while it runs. We cannot ship that library ourselves, SEGGER's
  licence does not allow it
- For people who want SEGGER's driver rather than probe-rs's

### TCP and Telnet

- Plain TCP client and Telnet, for ser2net, ESP-Link and other network bridges

---

## Views

Every session can switch view at any time. The data is the same, only shown
differently. A split shows two views side by side, of one session (terminal and hex)
or of two (UART and RTT).

### Terminal view

- xterm compatible, which covers VT100, VT102, VT220 and ANSI escape codes
- 16, 256 and 24-bit color; bold, italic, underline and the rest
- UTF-8 by default; ASCII and Latin-1 as options
- What a received CR or LF means (CR, LF, or both as a new line)
- Backspace sends BS or DEL; local echo on or off; bell as sound, flash or off
- Scrollback size, cursor style, font, size and color theme
- Clickable links; copy on select as an option
- Ctrl+Shift+C and Ctrl+Shift+V copy and paste (Cmd+C and Cmd+V on macOS), so
  Ctrl+C still sends 0x03 to the device
- Asks before pasting many lines at once

### Log view

- One row per line, with a timestamp: absolute, since connect, or since the line before
- Regex search with next and previous, and a count of matches
- Filters: show only lines that match, or hide lines that match
- Highlight rules: a pattern and a color, such as "ERROR" in red
- Bookmarks on lines, and jump between them
- Scrolls smoothly through millions of lines

### Hex view

- Hex and ASCII side by side, with the offset
- Choose bytes per row; group by 1, 2, 4 or 8 bytes
- Received and sent bytes in different colors
- Select bytes to see them as numbers: int8 to int64, float and double, both endians

### Plot view

See [Plotter and dashboard](#plotter-and-dashboard) below.

---

## Zephyr

- **Shell:** works in the terminal view over UART or RTT (`CONFIG_SHELL_BACKEND_SERIAL`
  or `CONFIG_SHELL_BACKEND_RTT`), with colors, tab completion and history
- **Logs:** recognises `[00:00:01.234,567] <inf> module: message`, with or without the
  timestamp and colors. Filter by level and by module, color by level
- **Dictionary logging** (later): decodes Zephyr's binary logs with the
  `log_dictionary.json` from the build
- **MCUmgr / SMP** (later), over serial or RTT:
  - firmware: list images, upload, test, confirm, reset
  - OS: task list with stack use and CPU, memory pool stats, echo
  - file system: browse, download and upload files (LittleFS, FAT)
  - shell commands through SMP

---

## Input

- Type straight into the terminal view
- Send box: one line, with history (up and down keys), line ending choice, sent as
  text or hex (`AA 55 01`), with escapes such as `\r`, `\n` and `\x03`
- Macro buttons per session, each with an optional repeat time
- Send a file, raw or line by line with a pause between lines

---

## Logging to file

- Raw bytes, or plain text with colors removed, with or without timestamps
- Each tab turns its log on or off (the Log button, File > Log this tab, Ctrl+Shift+L);
  new tabs can start with it on (Settings > Logging)
- Every log is a new file named after the tab and the time, in a folder of your choice
  ("ternima logs" in Documents unless set)
- A marker in the file when the device connects, disconnects and comes back
- File > Save output as saves what the terminal shows, scrollback and all
- Later: a new file by size or by day

---

## Saving and restoring

Two files:

- `settings.toml`: your preferences: theme, font, keys, defaults for new sessions
- `workspace.toml`: what is open: tabs, splits, each session's full settings, window
  size and place, command history and macros

They are saved on every change, about a second after the last one, not only when the
app closes, so a crash or a power cut loses nothing. Each save goes to a temporary
file first and then replaces the old one, so a half written file never replaces a
good one.

| OS | Folder |
|---|---|
| Windows | `%APPDATA%\com.nimaltd.ternima\` |
| Linux | `~/.config/com.nimaltd.ternima/` |
| macOS | `~/Library/Application Support/com.nimaltd.ternima/` |

**Portable mode:** if a file called `ternima.portable` sits next to the program,
the config goes into a `config` folder beside it instead. Handy on a USB stick.

### Finding the same port again

A session remembers more than the port name, because COM5 can come back as COM7:

```toml
[[session]]
name = "Nucleo UART"
kind = "serial"
view = "terminal"

[session.serial]
port = "COM5"
usb = { vid = 0x0483, pid = 0x374B, serial = "066DFF495150897267121441", interface = 2 }
baud = 115200
data_bits = 8
parity = "none"
stop_bits = 1
flow = "none"
dtr = true
rts = false
```

When ternima starts, or a device is plugged in, it looks for each session's port in
this order:

1. Same VID, PID, serial number and USB interface. The name may have changed; that
   is fine
2. Adapters without a serial number (many CH340s): same VID and PID and the same
   name, or else the only port with that VID and PID
3. Ports that are not USB, such as `/dev/ttyS0` or `/dev/ttyAMA0`: same name

If nothing matches, the tab shows "Waiting for Nucleo UART..." and connects as soon
as the device appears.

When a connected device goes away (board reset, cable pulled), the session waits for
it and reconnects by itself, and puts a line in the terminal and in the log file:

```
---- disconnected 14:02:11.204, reconnected 14:02:13.881 (2.7 s) ----
```

An RTT session is found again by the probe's serial number and the chip.

---

## Plotter and dashboard

### Plotter

Draws numbers from the data as live charts, like an oscilloscope.

- Reads numbers from:
  - plain lines: `1.2, 3.4, 5` or `1.2 3.4 5`
  - named values: `temp=21.5 hum=40` or `temp:21.5,hum:40` (the Arduino Serial
    Plotter format, so Arduino sketches plot with no change)
  - JSON lines: `{"temp": 21.5, "hum": 40}`
  - your own regex, one number per capture group
  - binary frames, through the frame decoder below
  - an RTT channel that carries raw numbers (int16, float and so on)
- Charts: value over time, X against Y, histogram, and FFT
- Rolling window or keep everything; pause; zoom; two cursors that show the time and
  value between them
- Trigger like a scope: wait for a rising or falling edge through a level, then hold
- Per series: color, scale, offset, hide; min, max and average
- Export the data as CSV and the chart as PNG

### Dashboard

A page of widgets built from the same values: number, gauge, bar, LED, text, and a
small chart. Buttons and sliders send commands back, such as `pwm {value}\r\n`.

---

## Probe tools

probe-rs gives us more than RTT. With the same probe, ternima can also:

- **Watch variables live** while the target runs, without changing the firmware:
  pick global variables by name from the ELF, ternima reads them through the probe
  many times a second and shows or plots them, like STM32CubeMonitor or J-Scope
- **Read peripheral registers** by name and bit field, from the chip's SVD file
- **Decode a HardFault:** read the fault registers and the stacked registers, and say
  in plain words what went wrong and where (function, file and line from the ELF)
- **Read SWO / ITM** output from probes that support it
- **Show semihosting** output
- Reset, halt and resume buttons

### Crash and address decoding

When a line in the data contains code addresses (a Zephyr fatal error, a HardFault
handler's print, an ESP32 backtrace), ternima looks them up in the session's ELF
file and adds the function name, file and line under it, as ESP-IDF's monitor does.
This works over plain serial too, no probe needed.

---

## Device and protocol tools

- **Frame decoder:** describe a binary frame (start bytes, length field, payload,
  CRC) and ternima splits the data into frames, checks the CRC and shows the fields.
  Ready made decoders for COBS, SLIP, Modbus RTU, NMEA (GPS) and MAVLink
- **Modbus master:** read and write coils and registers over RS-485, poll a table of
  registers, scan for devices
- **Spy mode:** open two ports, one on each wire of a UART link between two devices,
  and see both directions in one timeline, colored by direction
- **Bridge:** connect a port to a TCP port (plain or RFC 2217), so another computer or
  another ternima can use the device over the network; or connect two ports together
- **Send and expect tests:** a list of steps such as "send `ver`, expect `v1.2` within
  2 s". Runs on one device or on many, and gives a pass or fail report. Useful for
  production testing
- **Record and replay:** save a session with its timing and play it back later, for
  testing parsers and plots without hardware
- **External tool buttons:** run a command such as `west flash` or `make flash`.
  ternima lets go of the port or probe while it runs, then reconnects
- **ESP32:** reset with DTR and RTS the way its boards wire them, and decode backtraces
- **AT commands:** a command list and response highlighting for ESP-AT and cellular
  modems

### Calculators

Small tools in a side panel:

- CRC calculator with the common CRC-8, CRC-16 and CRC-32 kinds, and your own
- Number converter: hex, decimal, binary, ASCII, float and double, both endians
- Baud rate error: for a clock and a baud rate, the real rate and the error
- ASCII table

---

## How it is built

### Layout

```
ternima/
  Cargo.toml              Rust workspace
  package.json            UI build (Vite)
  crates/
    ternima-core/        everything without UI: transports, sessions, config,
                          logging, parsers. Tested on its own
  src-tauri/              the Tauri app: connects ternima-core to the UI
  ui/                     Svelte 5 + TypeScript + xterm.js + uPlot
  docs/                   this plan and the user guide
  .github/workflows/      CI and releases
```

### How data flows

```
device -> transport -> session task -+-> log file (every byte, always)
                                     +-> scrollback, kept in Rust
                                     +-> parsers (lines, Zephyr, plot values, frames)
                                     +-> UI, in batches about 60 times a second

keyboard and send box -> session task -> transport -> device
```

- Each session runs on its own task. Its read loop never waits for the UI or the disk
- The UI gets data in batches, so the screen stays smooth even at megabytes per
  second. If the UI falls behind, the batches get bigger; nothing is dropped
- The log view does not keep every line in the web view. Lines stay in Rust and the
  UI asks for the rows it shows. That is how it scrolls through millions of lines

### Libraries

| Need | Library |
|---|---|
| Serial ports | `serialport` |
| Probes, RTT, memory | `probe-rs` |
| ELF and debug info | `object`, `gimli`, `addr2line` |
| Config | `serde`, `toml` |
| Async | `tokio` (comes with Tauri) |
| Terminal | `@xterm/xterm` with the fit, search, web links, unicode and WebGL addons |
| Charts | `uplot` |
| UI | Svelte 5, Vite, TypeScript |

---

## Platform notes

- **Windows:** WebView2 is part of Windows 10 and 11; the installer adds it if it is
  missing. Drivers for CH340, CP210x and FTDI adapters usually come from Windows
  Update. CMSIS-DAP needs no driver. probe-rs may need a J-Link switched to the WinUSB
  driver (J-Link Configurator does it); the SEGGER library route avoids that
- **Linux:** serial ports need your user in the `dialout` group (Debian, Ubuntu) or
  `uucp` (Arch). Probes need udev rules; we ship them and the app tells you when
  access is denied. ModemManager can grab new serial ports for a few seconds and send
  AT commands to them; the guide shows how to stop that
- **macOS:** USB CDC and FTDI need no driver. Until we sign and notarize the app, the
  first start needs a right click and Open (signing needs an Apple Developer account)

---

## Builds and releases

- **CI on every push and pull request:** `cargo fmt --check`, `cargo clippy`, the tests
  on Windows, Linux and macOS, a type check of the UI, and a build of the app
- **Release on a tag** `vX.Y.Z`: GitHub Actions builds the installers and puts them on
  a draft GitHub Release

| OS | Files |
|---|---|
| Windows | `.msi`, setup `.exe`, portable `.zip` |
| macOS | `.dmg` (one file for Apple Silicon and Intel) |
| Linux | `.AppImage`, `.deb`, `.rpm` |

Later: code signing on Windows and macOS, auto update from GitHub Releases, and
listings in winget, Homebrew and Flathub. No package is called `ternima` in the AUR,
on crates.io or on npm (checked in October 2026), so it can keep its name there.

---

## Testing

- Unit tests in `ternima-core`: config load and save, port matching, line splitting,
  the Zephyr parser, plot value parsing, frame decoding
- A fake transport that plays back recorded data, to test sessions and reconnecting
  without hardware
- On Linux CI, virtual serial port pairs (`socat`) to test the real serial code
- Before each release, a hardware check: one board per probe kind (DAPLink, J-Link,
  ST-Link), the common USB serial chips (CH340, CP2102, FTDI, USB CDC), and the Zephyr
  shell over UART and over RTT

---

## Risks

- **WebKitGTK on Linux** is slower than WebView2 and Safari, and its WebGL is not
  always reliable. xterm.js falls back to its normal renderer. We test on Ubuntu and
  Fedora
- **One program per probe:** probe-rs cannot share a probe with a running debugger.
  ternima says so clearly and offers RTT over TCP
- **J-Link on Windows** with probe-rs may need the WinUSB driver, until the SEGGER
  library route is done
- **macOS shortcuts** have not been tried on a Mac yet. macOS's own app menu may
  catch Cmd+C, Cmd+V and Cmd+A before the in-window menu sees them
- **The name:** the project started as ternimal, but another project has that name
  (github.com/p-e-w/ternimal, a small animation program under GPL-3.0, inactive since
  2018) and an Arch package of it, so it is ternima now. We always show it with a
  tagline, and put serial, RTT, J-Link and Zephyr in the repository description and
  topics

---

## Roadmap

### Phase 0: skeleton

- [x] Plan, README, LICENSE.md, NOTICE, CHANGELOG
- [x] Rust workspace with `ternima-core` and the Tauri app
- [x] A window with xterm.js showing a colored welcome text
- [x] Bundled fonts, dark and light themes
- [x] CI that builds and tests on Windows, Linux and macOS
- [ ] Release workflow that makes installers from a tag (written; not run on GitHub yet)

Done when the installers from CI open the same window on all three OSes.

### Phase 1: serial

- [x] Menu bar in the window, with keyboard use (Alt+letter, arrows, Enter, Escape)
- [x] Copy, paste, select all and clear screen: Ctrl+Shift+C, V, A and K (Cmd on macOS)
- [x] Text size (Ctrl+=, Ctrl+-, Ctrl+0) and theme (system, dark, light) in the View menu
- [x] `settings.toml`, saved on every change; a broken file is kept as `settings.bad.toml`
- [x] About box
- [x] Port list with USB name, VID:PID and serial number, updated on plug and unplug
- [x] Serial options: baud, data bits, parity, stop bits, flow control; DTR and RTS
  buttons; break; modem line lights. Settings change on an open port without closing the tab
- [x] Terminal view with typing, Enter as CR, LF or CR LF, local echo, new line on LF
- [x] `workspace.toml` with the tabs and sessions, saved on every change; tabs that were
  connected connect again at start
- [x] Finding the same port again, "Waiting for...", reconnecting with a marker line
- [x] Clear messages when a port cannot be opened, naming the tab when ternima itself has it
- [x] Tabs, with Ctrl+Tab and Ctrl+Shift+Tab, and a state dot on each
- [x] Status bar: state, time connected, bytes in and out
- [x] Logging to file: per tab on or off, text or raw, timestamps, markers; a Settings
  window with the logging options; File > Save output as and Open log folder
- [x] Window size and position kept in `workspace.toml`, maximized too; a window whose
  screen was unplugged opens in the middle of the main screen
- [ ] Text encodings other than UTF-8
- [ ] Unplug and replug tried by hand on Windows, Linux and macOS (the reconnect logic is
  covered by tests with pretend ports)

Done when it replaces PuTTY or Tera Term for everyday UART work.

### Phase 2: RTT and network

- [x] Probe list (CMSIS-DAP, J-Link, ST-Link) with serial numbers, updated on plug and unplug
- [x] Chip search over probe-rs's chip list; the name is checked before connecting
- [x] ELF file or RAM search for the control block, or a fixed address; up and down
  channel choice; SWD speed; attach without reset, or under reset
- [x] Reconnecting after a probe unplug, and finding the control block again after a
  target reset or new firmware; a Reset target button
- [x] RTT tabs saved in `workspace.toml` next to serial ones, each with only its own settings
- [x] Tried on an STM32H503RB through an ST-Link V3: attach, RAM search, clear message
  when the firmware has no RTT
- [x] RTT data on real hardware: a test firmware on the H503 with a shell on channel 0
  and log lines on channel 1; typing, channel switching and Reset target all tried
- [x] SWO, brought forward from Phase 5: ITM decoding, a stimulus port or all ports,
  core clock and baud checked, reconnect, Reset target. Tried on the H503 at 2 MHz.
  probe-rs turns the STM32H5 trace pin on at the wrong address, so ternima also sets
  it at 0x44024004 (worth reporting to probe-rs)
- [x] Finding the chip by itself: an STM32's DBGMCU ID register gives its family (the H503
  answers DEV_ID 0x474), and probe-rs's own detection covers the chips it can tell apart.
  The Detect button, and the dialog by itself when the chip is empty
- [x] RTT over TCP: J-Link's RTT server on 19021, with its channel asked for in the
  first 100 ms, and OpenOCD's `rtt server`. Tried against a stand-in J-Link server;
  not yet against real J-Link software or OpenOCD
- [x] Plain TCP and Telnet sessions, waiting for the other end and connecting again when
  it comes back. Telnet answers the option talk (echo and no go ahead yes, the rest no)
  and keeps it out of the terminal
- [x] Split view: two tabs side by side or one above the other, with a line to drag between
  them, kept in `workspace.toml`
- [x] Find in the terminal output: next and previous, match case, whole word, regex, and a
  count of matches
- [x] Startup settings: reopen the tabs, connect the ones that were connected, and open the
  window where it was, each one on or off

Done when the Zephyr shell works over RTT on a DAPLink and on a J-Link.

### Phase 3: log, hex and input

- [x] Log view (the Lines view): every line in a list of up to 100,000, with search (text or
  regex, only the matching lines if wanted), highlight rules in six colors, and bookmarks
- [x] Zephyr log parsing with level and module filters: time, level and module read from
  `[time] <level> module: message`, colors taken out first, a LOG_HEXDUMP's indented lines
  kept with their log line
- [x] Hex view and sending hex: sent and received bytes, 8, 16 or 32 to a line, with a new
  line after a chosen byte
- [x] A new line after a chosen pause, in the terminal and the hex view, but not in a
  shell's echo
- [ ] Pauses measured where the bytes are read, for 1 to 2 ms instead of 50 ms at least
- [x] Send box with history, macros (Alt+1 to Alt+9), escapes such as `\r` and `\x1b`,
  and sending a file paced for the link, with progress and Stop
- [x] Command palette: every menu command, the tabs and the macros, found by a few letters

Done when you can find one module's errors in a busy Zephyr log in seconds.

### Phase 4: plotter and dashboard

- [x] Value parsing: plain, named, Arduino, JSON, regex
- [x] Time chart with zoom, pause, cursors and stats
- [x] XY chart, histogram, FFT, scope style trigger
- [x] CSV and PNG export
- [ ] Values from binary frames and from raw RTT channels (int16, float...)
- [ ] Dashboard widgets, with buttons and sliders that send commands
- [ ] Record and replay

### Phase 5: probe tools

- [ ] Crash and address decoding from the ELF
- [ ] Live variable watch and plot
- [ ] HardFault decoder
- [ ] SVD register viewer
- [x] SWO / ITM (done in Phase 2)
- [ ] Semihosting

### Phase 6: device and protocol tools

- [ ] Frame decoder with COBS, SLIP, Modbus RTU, NMEA and MAVLink
- [ ] Modbus master
- [ ] Spy mode and bridge (TCP, RFC 2217, port to port)
- [ ] Send and expect tests
- [ ] External tool buttons
- [ ] MCUmgr / SMP; Zephyr dictionary logging; defmt
- [ ] Calculators
- [ ] SEGGER J-Link library

### Phase 7: 1.0

- [ ] Code signing on Windows and macOS
- [ ] Portable `.zip` for Windows
- [ ] Auto update
- [ ] User guide
- [ ] winget, Homebrew and Flathub
- [ ] Command line mode: `ternima --port COM3 -b 115200`
- [ ] Scripting (Rhai or Lua) and triggers (on a pattern: beep, notify, send a command)
