//! Telnet, as far as a terminal needs it: the option talk is answered and kept
//! out of the data, and data bytes that look like commands are escaped.

/// "Interpret as command": the byte in front of every Telnet command.
const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
/// Starts a subnegotiation, which runs until IAC SE.
const SB: u8 = 250;
const SE: u8 = 240;

/// Options: 8-bit data, the other end echoing, and no "go ahead" signals.
const BINARY: u8 = 0;
const ECHO: u8 = 1;
const SUPPRESS_GO_AHEAD: u8 = 3;

/// Where the decoder is in the byte stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Data,
    /// Just after a CR, where a NUL only marks the CR as a bare one.
    Cr,
    /// Just after IAC.
    Command,
    /// Just after IAC and WILL, WONT, DO or DONT: the option comes next.
    Option(u8),
    /// Inside a subnegotiation, which is skipped.
    Sub,
    /// Just after IAC inside a subnegotiation.
    SubCommand,
}

/// One connection's Telnet state.
pub struct Telnet {
    state: State,
    /// The options the other end does, and the ones this end does.
    remote: [bool; 256],
    local: [bool; 256],
}

impl Default for Telnet {
    fn default() -> Self {
        Self {
            state: State::Data,
            remote: [false; 256],
            local: [false; 256],
        }
    }
}

impl Telnet {
    /// Takes received bytes: the data goes to `data`, and answers to the
    /// other end's option requests go to `reply`, to be sent back.
    pub fn receive(&mut self, input: &[u8], data: &mut Vec<u8>, reply: &mut Vec<u8>) {
        for &byte in input {
            self.state = match self.state {
                State::Data | State::Cr => match byte {
                    IAC => State::Command,
                    // CR NUL is a CR on its own.
                    0 if self.state == State::Cr => State::Data,
                    b'\r' => {
                        data.push(byte);
                        State::Cr
                    }
                    _ => {
                        data.push(byte);
                        State::Data
                    }
                },
                State::Command => match byte {
                    IAC => {
                        data.push(IAC);
                        State::Data
                    }
                    WILL | WONT | DO | DONT => State::Option(byte),
                    SB => State::Sub,
                    // NOP, go ahead and the other commands change nothing here.
                    _ => State::Data,
                },
                State::Option(verb) => {
                    self.answer(verb, byte, reply);
                    State::Data
                }
                State::Sub => match byte {
                    IAC => State::SubCommand,
                    _ => State::Sub,
                },
                State::SubCommand => match byte {
                    SE => State::Data,
                    _ => State::Sub,
                },
            };
        }
    }

    /// Gets bytes ready to send: 0xFF is doubled, and a CR without an LF
    /// after it gets a NUL, as Telnet wants, unless 8-bit data was agreed.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 2);
        for (i, &byte) in data.iter().enumerate() {
            match byte {
                IAC => out.extend([IAC, IAC]),
                b'\r' if !self.local[BINARY as usize] && data.get(i + 1) != Some(&b'\n') => {
                    out.extend([b'\r', 0]);
                }
                _ => out.push(byte),
            }
        }
        out
    }

    /// Answers an option request. Each change is answered once, so the two
    /// ends never loop.
    fn answer(&mut self, verb: u8, option: u8, reply: &mut Vec<u8>) {
        let at = option as usize;
        match verb {
            WILL if accepts_remote(option) && !self.remote[at] => {
                self.remote[at] = true;
                reply.extend([IAC, DO, option]);
            }
            WILL if !accepts_remote(option) => reply.extend([IAC, DONT, option]),
            WONT if self.remote[at] => {
                self.remote[at] = false;
                reply.extend([IAC, DONT, option]);
            }
            DO if accepts_local(option) && !self.local[at] => {
                self.local[at] = true;
                reply.extend([IAC, WILL, option]);
            }
            DO if !accepts_local(option) => reply.extend([IAC, WONT, option]),
            DONT if self.local[at] => {
                self.local[at] = false;
                reply.extend([IAC, WONT, option]);
            }
            // Already as asked.
            _ => {}
        }
    }
}

/// What the other end may do: send 8-bit data, echo what is typed, and leave
/// out "go ahead", which together make a character at a time terminal.
fn accepts_remote(option: u8) -> bool {
    matches!(option, BINARY | ECHO | SUPPRESS_GO_AHEAD)
}

/// What this end does when asked. It does not echo: the terminal's own
/// local echo setting does that.
fn accepts_local(option: u8) -> bool {
    matches!(option, BINARY | SUPPRESS_GO_AHEAD)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERMINAL_TYPE: u8 = 24;

    /// Feeds chunks to a decoder, and gives back the data and the replies.
    fn feed(telnet: &mut Telnet, chunks: &[&[u8]]) -> (Vec<u8>, Vec<u8>) {
        let (mut data, mut reply) = (Vec::new(), Vec::new());
        for chunk in chunks {
            telnet.receive(chunk, &mut data, &mut reply);
        }
        (data, reply)
    }

    #[test]
    fn plain_text_passes_through() {
        let (data, reply) = feed(&mut Telnet::default(), &[b"uart:~$ help\r\n"]);
        assert_eq!(data, b"uart:~$ help\r\n");
        assert!(reply.is_empty());
    }

    #[test]
    fn doubled_iac_is_one_data_byte() {
        let (data, _) = feed(&mut Telnet::default(), &[&[b'a', IAC, IAC, b'b']]);
        assert_eq!(data, [b'a', 0xFF, b'b']);
    }

    #[test]
    fn echo_and_suppress_go_ahead_are_accepted_once() {
        let mut telnet = Telnet::default();
        let (data, reply) = feed(
            &mut telnet,
            &[&[IAC, WILL, ECHO, IAC, WILL, SUPPRESS_GO_AHEAD], b"login: "],
        );
        assert_eq!(data, b"login: ");
        assert_eq!(reply, [IAC, DO, ECHO, IAC, DO, SUPPRESS_GO_AHEAD]);
        // Asked again, it is already agreed, so nothing more is said.
        let (_, again) = feed(&mut telnet, &[&[IAC, WILL, ECHO]]);
        assert!(again.is_empty());
    }

    #[test]
    fn options_this_end_does_not_do_are_refused() {
        let (_, reply) = feed(
            &mut Telnet::default(),
            &[&[IAC, DO, TERMINAL_TYPE, IAC, DO, SUPPRESS_GO_AHEAD]],
        );
        assert_eq!(
            reply,
            [IAC, WONT, TERMINAL_TYPE, IAC, WILL, SUPPRESS_GO_AHEAD]
        );
    }

    #[test]
    fn a_turned_off_option_is_answered_once() {
        let mut telnet = Telnet::default();
        feed(&mut telnet, &[&[IAC, WILL, ECHO]]);
        let (_, reply) = feed(&mut telnet, &[&[IAC, WONT, ECHO, IAC, WONT, ECHO]]);
        assert_eq!(reply, [IAC, DONT, ECHO]);
    }

    #[test]
    fn subnegotiation_is_skipped() {
        let (data, reply) = feed(
            &mut Telnet::default(),
            &[&[b'a', IAC, SB, TERMINAL_TYPE, 1, IAC, IAC, IAC, SE, b'b']],
        );
        assert_eq!(data, b"ab");
        assert!(reply.is_empty());
    }

    #[test]
    fn commands_split_across_reads_still_work() {
        let (data, reply) = feed(
            &mut Telnet::default(),
            &[&[b'x', IAC], &[WILL], &[ECHO, b'y']],
        );
        assert_eq!(data, b"xy");
        assert_eq!(reply, [IAC, DO, ECHO]);
    }

    #[test]
    fn cr_nul_is_a_bare_cr() {
        let (data, _) = feed(&mut Telnet::default(), &[b"a\r\0b\r", b"\nc"]);
        assert_eq!(data, b"a\rb\r\nc");
    }

    #[test]
    fn sending_escapes_iac_and_bare_cr() {
        let telnet = Telnet::default();
        assert_eq!(telnet.encode(b"ls\r"), b"ls\r\0");
        assert_eq!(telnet.encode(b"ls\r\n"), b"ls\r\n");
        assert_eq!(telnet.encode(&[1, IAC, 2]), [1, IAC, IAC, 2]);
    }

    #[test]
    fn binary_mode_sends_cr_as_it_is() {
        let mut telnet = Telnet::default();
        feed(&mut telnet, &[&[IAC, DO, BINARY]]);
        assert_eq!(telnet.encode(b"ls\r"), b"ls\r");
    }
}
