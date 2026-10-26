# 📟 terNIMA

[![CI](https://github.com/nimaltd/ternima/actions/workflows/ci.yml/badge.svg)](https://github.com/nimaltd/ternima/actions/workflows/ci.yml)
[![Stars](https://img.shields.io/github/stars/nimaltd/ternima?style=social)](https://github.com/nimaltd/ternima)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE.md)

A serial, RTT, SWO and TCP terminal for embedded developers, the same on Windows, macOS and Linux.

The name is "terminal" with my name, Nima, in it: ter-NIMA.

![terNIMA showing Zephyr logs by level and module, with highlights](https://nimaltd.github.io/assets/ternima-logs.png)

More of it, feature by feature, is on [nimaltd.github.io](https://nimaltd.github.io/#ternima).

---

## ⬇️ Download

terNIMA for Windows, macOS and Linux is on the [Releases](https://github.com/nimaltd/ternima/releases/latest) page:

| OS | File |
|---|---|
| Windows 10 and 11, x64 | the `-setup.exe` installer, or the `.msi` |
| macOS, Apple Silicon and Intel | the `.dmg` |
| Linux, x64 | the `.AppImage`, or the `.deb` or `.rpm` |

The apps are not signed yet, so the first time:

- **Windows** may say "Windows protected your PC": click **More info**, then **Run anyway**.
- **macOS** may say the app is damaged or cannot be checked: move it to Applications, then run `xattr -cr /Applications/terNIMA.app` in Terminal.
- **Linux**: make the AppImage runnable with `chmod +x`, and to use serial ports, add yourself to the `dialout` group (`uucp` on Arch).

What changed in each version is in [CHANGELOG.md](CHANGELOG.md).

---

## ✨ What it does

- Serial ports with every option: any baud rate, data bits, parity, stop bits, flow control, DTR, RTS and break
- SEGGER RTT through CMSIS-DAP, J-Link and ST-Link probes, with no vendor software needed
- SWO through the same probes: ITM output such as printf, from one stimulus port or all of them
- RTT over TCP while your IDE is debugging, from J-Link or OpenOCD
- TCP and Telnet sessions, for devices and serial bridges on the network
- A send box with history, macros on Alt+1 to Alt+9, hex sending, and sending a file
- A full VT100 / xterm terminal with 256 and 24-bit color, so the Zephyr shell works as it should
- Zephyr logs you can filter by level and module
- Log, hex and plot views of the same data, side by side if you like
- A live plotter for the numbers in the lines, Arduino Serial Plotter format included: over time with cursors, stats and a scope trigger, XY, histogram and FFT
- Record a session and play it back later with its timing, faster or slower, from any point
- A Modbus RTU and ASCII tester on serial ports, as a master that polls and plots, or as a slave that answers
- Every Modbus frame on the bus in a table, each response under its request, checked, with the time it took
- Remembers everything when you close it, finds your device again even if its COM number changed, and reconnects when you plug it back in

---

## 🛠️ Building it

terNIMA is written in Rust and TypeScript with [Tauri](https://tauri.app). With Rust, Node.js and Tauri's prerequisites installed:

```bash
npm install
npm run tauri dev
```

[CONTRIBUTING.md](CONTRIBUTING.md) lists what to install on each OS, how to make the installers, and the checks CI runs.

---

## 🤝 Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Nothing to sign, just open a pull request.

---

## 💖 Support

I write these tools in my own time and give them away, because good tools should be easy to get. If this one saved you an afternoon, there are two things that genuinely help:

**⭐ Star the repo.** It costs you one click, it helps other engineers find the project, and it is the main reason I keep going.

**☕ [Buy me a coffee on Ko-fi](https://ko-fi.com/nimaltd).** Any amount is a real motivation to keep writing, documenting and maintaining this work.

[![GitHub](https://img.shields.io/badge/GitHub-Follow-black?style=for-the-badge&logo=github)](https://github.com/NimaLTD)
[![YouTube](https://img.shields.io/badge/YouTube-Subscribe-red?style=for-the-badge&logo=youtube)](https://youtube.com/@nimaltd)
[![Instagram](https://img.shields.io/badge/Instagram-Follow-purple?style=for-the-badge&logo=instagram)](https://instagram.com/github.nimaltd)
[![LinkedIn](https://img.shields.io/badge/LinkedIn-Connect-blue?style=for-the-badge&logo=linkedin)](https://linkedin.com/in/nimaltd)
[![Email](https://img.shields.io/badge/Email-Contact-red?style=for-the-badge&logo=gmail)](mailto:nima.askari@gmail.com)
[![Ko-fi](https://img.shields.io/badge/Ko--fi-Support-orange?style=for-the-badge&logo=ko-fi)](https://ko-fi.com/nimaltd)

---

## 📜 License

Apache License 2.0. See [LICENSE.md](LICENSE.md).

You are free to use this in commercial and closed source products. What the license asks in return is that you keep the copyright notice and pass along the [NOTICE](NOTICE) file, so the credit travels with the code.
