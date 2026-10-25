// The TCP types of ternima-core, as the UI sees them: plain sockets, Telnet,
// and the RTT servers of J-Link's software and OpenOCD.

export type Protocol = "raw" | "telnet" | "jlink" | "openocd";

export type TcpConfig = {
  host: string;
  port: number;
  protocol: Protocol;
  /** The RTT channel J-Link's server uses both ways. */
  channel: number;
};

export type NetState =
  | { state: "waiting"; problem: string | null }
  | { state: "connected"; address: string; protocol: Protocol };

export const DEFAULT_TCP: TcpConfig = { host: "localhost", port: 0, protocol: "raw", channel: 0 };

/** Each protocol's name, usual port, and what to know about it. */
export const PROTOCOLS: Record<Protocol, { name: string; port: number | null; hint: string }> = {
  raw: {
    name: "TCP",
    port: null,
    hint: "Plain bytes both ways, as from a serial to network bridge or a device's own socket.",
  },
  telnet: {
    name: "Telnet",
    port: 23,
    hint: "A device's shell or a terminal server that speaks Telnet. Its option talk is answered for you.",
  },
  jlink: {
    name: "J-Link RTT",
    port: 19021,
    hint:
      "J-Link's software opens this port while it is connected to the chip: J-Link Commander, " +
      "GDB Server, Ozone, RTT Viewer or a debugger in your IDE. The debugger keeps the probe.",
  },
  openocd: {
    name: "OpenOCD RTT",
    port: 9090,
    hint:
      'Start it in OpenOCD: rtt setup <RAM address> <RAM size> "SEGGER RTT", then rtt start, ' +
      "then rtt server start 9090 0 for channel 0. The debugger keeps the probe.",
  },
};

/** "localhost:19021", or "[::1]:23" for an IPv6 address. */
export function address(config: TcpConfig): string {
  const host = config.host.trim();
  return host.includes(":") && !host.startsWith("[") ? `[${host}]:${config.port}` : `${host}:${config.port}`;
}

/** "J-Link RTT, channel 1", "Telnet" */
export function protocolText(config: TcpConfig): string {
  const name = PROTOCOLS[config.protocol].name;
  return config.protocol === "jlink" && config.channel !== 0 ? `${name}, channel ${config.channel}` : name;
}

/** Whether the other end is a debugger's RTT server. */
export function isRttServer(config: TcpConfig): boolean {
  return config.protocol === "jlink" || config.protocol === "openocd";
}
