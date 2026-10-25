# 📟 ternima

[![CI](https://github.com/nimaltd/ternima/actions/workflows/ci.yml/badge.svg)](https://github.com/nimaltd/ternima/actions/workflows/ci.yml)
[![Stars](https://img.shields.io/github/stars/nimaltd/ternima?style=social)](https://github.com/nimaltd/ternima)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE.md)

A serial, RTT and Zephyr terminal for embedded developers, the same on Windows, Linux and macOS.

The name is "terminal" with my name, Nima, in it: ter-NIMA.

> **Status:** early development. There is nothing to download yet. What it will do, what it is built with and the order it is built in are all in [docs/PLAN.md](docs/PLAN.md).

---

## ✨ What it will do

- Serial ports with every option: any baud rate, data bits, parity, stop bits, flow control, DTR, RTS and break
- SEGGER RTT through CMSIS-DAP, J-Link and ST-Link probes, with no vendor software needed
- RTT over TCP while your IDE is debugging, from J-Link or OpenOCD
- A full VT100 / xterm terminal with 256 and 24-bit color, so the Zephyr shell works as it should
- Zephyr logs you can filter by level and module
- Log, hex and plot views of the same data, side by side if you like
- A live plotter and dashboard, Arduino Serial Plotter format included
- Probe tools: flash firmware, watch variables live, decode HardFaults and crash addresses from your ELF
- Remembers everything when you close it, finds your device again even if its COM number changed, and reconnects when you plug it back in

---

## 🗺️ Roadmap

| Phase | What |
|---|---|
| 0 | Skeleton: the app window and builds for all three OSes |
| 1 | Serial |
| 2 | RTT and network |
| 3 | Log, hex and input |
| 4 | Plotter and dashboard |
| 5 | Probe tools |
| 6 | Device and protocol tools |
| 7 | 1.0 |

Each phase's checklist is in [docs/PLAN.md](docs/PLAN.md#roadmap).

---

## 🛠️ Building it

ternima is written in Rust and TypeScript with [Tauri](https://tauri.app). With Rust, Node.js and Tauri's prerequisites installed:

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
