// The SSH types of ternima-core, as the UI sees them.

import { address } from "./net";

/** How a session logs in. A password is asked each time, so it is never in the settings. */
export type SshAuth = "password" | "key" | "agent";

export type SshConfig = {
  host: string;
  port: number;
  user: string;
  auth: SshAuth;
  /** The private key file, for a key login. */
  key_file?: string;
};

/** The port SSH servers listen on, unless told otherwise. */
export const SSH_PORT = 22;

export const DEFAULT_SSH: SshConfig = { host: "", port: SSH_PORT, user: "", auth: "password", key_file: "" };

export type SshState =
  | { state: "waiting"; problem: string | null }
  /** A shell is open at `address`: "root@board:22". */
  | { state: "connected"; address: string }
  /** Not connected, and not trying again until told to. */
  | { state: "ended"; why: string };

/** What only the user can say. The session waits for the answer. */
export type SshQuestion =
  /** known_hosts has no key of this type for this server: trust this one? */
  | { ask: "host_key"; address: string; key_type: string; fingerprint: string; others: string[]; file: string }
  /** The server shows another key than the one saved, so the session will not connect. */
  | {
      ask: "key_changed";
      address: string;
      key_type: string;
      fingerprint: string;
      known: string;
      file: string;
      /** The command that takes the old key out of the file. */
      forget: string;
    }
  /** A password, a passphrase, or what the server itself asks. */
  | { ask: "secret"; title: string; prompt: string; echo: boolean; problem: string | null; note: string };

export type SshAnswer = { answer: "yes" } | { answer: "no" } | { answer: "text"; text: string };

/** "root@board:22" */
export function sshTarget(config: SshConfig): string {
  return `${config.user.trim()}@${address(config)}`;
}

/** What each way of logging in is, for the dialog. */
export const SSH_AUTH_HINTS: Record<SshAuth, string> = {
  password: "Asked each time you connect, also when the tab connects again. It is never saved.",
  key: "An OpenSSH or PuTTY private key, with its public half in ~/.ssh/authorized_keys on the server. A passphrase is asked each time and never saved.",
  agent:
    "The keys in OpenSSH's agent, added with ssh-add, or in Pageant on Windows. Nothing else is needed here.",
};
