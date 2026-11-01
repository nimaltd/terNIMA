//! One SSH session: a shell in a real terminal, kept open on its own thread,
//! and connected again when the connection breaks.
//!
//! What only the user can say, such as whether to trust a new server or a
//! password, comes back through [`Event::Ask`], and the session waits for
//! [`Session::answer`]. It holds a password only as long as it takes to send it.

use std::ffi::OsString;
use std::future::{Future, pending};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{
    self, Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate,
};
use russh::{
    AlgorithmKind, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect, MethodKind,
    MethodSet, Preferred, Pty, SshId, cipher, kex, mac,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::spawn_blocking;
use tokio::time::{Instant, sleep, sleep_until, timeout};

use super::config::{Auth, SshConfig};
use super::known_hosts::{self, Verdict};
use crate::net::{addresses, is_not_there};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SshState {
    /// Not connected, and trying. `problem` says why when something is wrong,
    /// such as a host name that cannot be found; without it, nothing answers yet.
    Waiting { problem: Option<String> },
    /// A shell is open at `address`, such as "root@board:22".
    Connected { address: String },
    /// Not connected, and not trying again until told to: the shell ended, the
    /// login failed or was cancelled, or the server cannot be trusted.
    Ended { why: String },
}

/// What only the user can say. The session waits for the answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Question {
    /// `known_hosts` has no key of this type for this server: trust this one?
    /// `others` are the types it has: a new type can mean someone is in between.
    HostKey {
        address: String,
        key_type: String,
        fingerprint: String,
        others: Vec<String>,
        file: String,
    },
    /// The server shows another key than the one in `known_hosts`, so the
    /// session will not connect. There is nothing to decide: any answer goes on.
    KeyChanged {
        address: String,
        key_type: String,
        fingerprint: String,
        known: String,
        file: String,
        /// The command that takes the old key out of the file.
        forget: String,
    },
    /// A password, a passphrase, or what the server itself asks.
    Secret {
        title: String,
        prompt: String,
        /// Whether what is typed can show, as the server says.
        echo: bool,
        /// Why it is asked again, if it is.
        problem: Option<String>,
        /// Where what is typed goes.
        note: String,
    },
}

/// The user's answer to a [`Question`]. It has no `Debug`, so a password cannot be printed.
#[derive(Clone, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    /// Trust the server's key.
    Yes,
    /// Do not go on.
    No,
    /// What was typed.
    Text { text: String },
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Bytes the shell printed, in the order they came.
    Data(Vec<u8>),
    State(SshState),
    Ask(Question),
}

enum Command {
    Write(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Reconfigure(SshConfig),
    Answer(Answer),
    Close,
}

/// How long things take. Tests make the times shorter.
#[derive(Clone, Copy)]
struct Timing {
    /// Between two tries to connect.
    retry: Duration,
    /// The longest one try to connect to one address takes.
    connect: Duration,
    /// The longest the server takes to say who it is and agree on the keys.
    /// A board with a slow CPU needs some seconds.
    handshake: Duration,
    /// The longest received bytes wait before they are passed on, so the
    /// screen updates about 60 times a second.
    batch: Duration,
    /// How often a quiet server is asked if it is there. After three times
    /// without an answer, the connection counts as broken.
    keepalive: Duration,
}

const TIMING: Timing = Timing {
    retry: Duration::from_secs(2),
    connect: Duration::from_secs(3),
    handshake: Duration::from_secs(20),
    batch: Duration::from_millis(16),
    keepalive: Duration::from_secs(15),
};

/// The most bytes passed on at once.
const BATCH_MAX: usize = 64 * 1024;

/// What the terminal says it is. xterm.js is a good xterm, and every system has this name.
const TERM: &str = "xterm-256color";

/// The Backspace key sends 127, and text is UTF-8.
const TERMINAL_MODES: [(Pty, u32); 2] = [(Pty::VERASE, 127), (Pty::IUTF8, 1)];

/// Where a password, or an answer to the server's own question, goes.
const TO_THE_SERVER: &str = "It goes to the server, and is never saved.";
/// Where a key file's passphrase goes.
const TO_THE_KEY_FILE: &str =
    "It only opens the key file here. It is not sent, and is never saved.";

#[cfg(windows)]
const NO_AGENT: &str = "No SSH agent answers. Start the OpenSSH Authentication Agent service and add a key with ssh-add, or start Pageant.";
#[cfg(not(windows))]
const NO_AGENT: &str = "No SSH agent answers. Start one with ssh-agent, so that SSH_AUTH_SOCK is set, and add a key with ssh-add.";

/// What small and old boards' servers may offer and nothing newer. These come
/// after russh's usual choices, so they are used only when nothing better is in common.
const OTHER_KEX: [kex::Name; 4] = [
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_G14_SHA1,
];
const OTHER_CIPHERS: [cipher::Name; 4] = [
    cipher::AES_128_GCM,
    cipher::AES_128_CBC,
    cipher::AES_192_CBC,
    cipher::AES_256_CBC,
];
const OTHER_MACS: [mac::Name; 2] = [mac::HMAC_SHA1_ETM, mac::HMAC_SHA1];

/// Where a session finds what it needs outside itself. Tests have their own.
#[derive(Clone)]
struct Places {
    home: PathBuf,
    known_hosts: PathBuf,
    /// The agent named by `SSH_AUTH_SOCK`, if any.
    agent: Option<OsString>,
}

impl Places {
    fn usual() -> Self {
        let home = std::env::home_dir().unwrap_or_default();
        let known_hosts = home.join(".ssh").join("known_hosts");
        Self {
            home,
            known_hosts,
            agent: std::env::var_os("SSH_AUTH_SOCK"),
        }
    }
}

/// An SSH session. Dropping it closes the connection.
pub struct Session {
    commands: UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts connecting and keeps the connection until the session is
    /// dropped, for a terminal of `size`, columns and rows. Everything that
    /// happens comes back through `on_event`, which runs on the session's own thread.
    pub fn start(
        config: SshConfig,
        size: (u32, u32),
        on_event: impl FnMut(Event) + Send + 'static,
    ) -> Self {
        Self::start_with(config, size, Box::new(on_event), TIMING, Places::usual())
    }

    fn start_with(
        config: SshConfig,
        size: (u32, u32),
        on_event: Box<dyn FnMut(Event) + Send>,
        timing: Timing,
        places: Places,
    ) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let worker = Worker {
            config,
            on_event,
            commands: receiver,
            timing,
            places,
            size,
            state: None,
            heard: Arc::default(),
        };
        let thread = thread::Builder::new()
            .name("ternima-ssh".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("the OS could not start what SSH needs");
                runtime.block_on(worker.run());
                // Not waiting for a host name still being looked up.
                runtime.shutdown_background();
            })
            .expect("the OS could not start a thread");
        Self {
            commands,
            thread: Some(thread),
        }
    }

    /// Sends bytes to the shell. They are dropped while no shell is open.
    pub fn write(&self, data: Vec<u8>) {
        self.send(Command::Write(data));
    }

    /// Tells the shell the terminal's new size, in columns and rows.
    pub fn resize(&self, cols: u32, rows: u32) {
        self.send(Command::Resize { cols, rows });
    }

    /// Connects again with new settings.
    pub fn reconfigure(&self, config: SshConfig) {
        self.send(Command::Reconfigure(config));
    }

    /// Gives the answer to the question the session is waiting on.
    pub fn answer(&self, answer: Answer) {
        self.send(Command::Answer(answer));
    }

    fn send(&self, command: Command) {
        // The thread only ends when the session is dropped, so this cannot fail before then.
        let _ = self.commands.send(command);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.send(Command::Close);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the server said while connecting and logging in, which the session
/// reads when its calls come back.
#[derive(Default)]
struct Heard {
    /// Its key, when `known_hosts` does not say to trust it.
    distrusted: Option<(PublicKey, Verdict)>,
    /// Why its key could not be looked up.
    problem: Option<String>,
    /// What it shows before the login.
    banner: String,
    /// Its words as it closed the connection.
    goodbye: Option<String>,
}

type SharedHeard = Arc<Mutex<Heard>>;

fn lock(heard: &SharedHeard) -> MutexGuard<'_, Heard> {
    heard.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Checks the server's key, and keeps what the server says for the session.
struct Client {
    known_hosts: PathBuf,
    host: String,
    port: u16,
    heard: SharedHeard,
}

impl client::Handler for Client {
    type Error = russh::Error;

    /// Never asks the user here: a key not trusted ends this connection, and
    /// the user has all the time they need, with the server not waiting.
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Certificates are not asked for, so none should come.
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };
        let mut heard = lock(&self.heard);
        match known_hosts::check(&self.known_hosts, &self.host, self.port, key) {
            Ok(Verdict::Known) => return Ok(true),
            Ok(verdict) => heard.distrusted = Some((key.clone(), verdict)),
            Err(problem) => heard.problem = Some(problem),
        }
        Ok(false)
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        lock(&self.heard).banner.push_str(banner);
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(info) => {
                lock(&self.heard).goodbye = Some(info.message);
                Ok(())
            }
            client::DisconnectReason::Error(error) => Err(error),
        }
    }
}

/// Why a try at a session ended.
enum Ended {
    /// The app closed the session.
    Closed,
    /// New settings, or a key just trusted: connect again now.
    Reconnect,
    /// The connection broke or cannot be made: tried again soon. With why,
    /// when something is wrong; without it, nothing answers yet.
    Lost(Option<String>),
    /// Not tried again, with why. Trying a login again and again could lock
    /// the account, and only the user can change what is wrong.
    Stopped(String),
}

/// What goes to the shell, on a task of its own: a window full of unread data
/// must not stop the session from reading what the shell prints.
enum Out {
    Data(Vec<u8>),
    Size(u32, u32),
}

async fn send_out(writer: ChannelWriteHalf<client::Msg>, mut queue: UnboundedReceiver<Out>) {
    while let Some(out) = queue.recv().await {
        let sent = match out {
            Out::Data(data) => writer.data_bytes(data).await,
            Out::Size(cols, rows) => writer.window_change(cols, rows, 0, 0).await,
        };
        if sent.is_err() {
            return;
        }
    }
}

struct Worker {
    config: SshConfig,
    on_event: Box<dyn FnMut(Event) + Send>,
    commands: UnboundedReceiver<Command>,
    timing: Timing,
    places: Places,
    /// The terminal's size, in columns and rows.
    size: (u32, u32),
    /// The state last told to the app, so each change is told once.
    state: Option<SshState>,
    /// What the server said on the connection being made.
    heard: SharedHeard,
}

impl Worker {
    async fn run(mut self) {
        loop {
            let ended = match self.try_session().await {
                Ok(ended) | Err(ended) => ended,
            };
            let go_on = match ended {
                Ended::Closed => false,
                Ended::Reconnect => {
                    // New settings: what it was connected to is gone, even if the address is the same.
                    if matches!(self.state, Some(SshState::Connected { .. })) {
                        self.set_state(SshState::Waiting { problem: None });
                    }
                    true
                }
                Ended::Lost(problem) => {
                    self.set_state(SshState::Waiting { problem });
                    self.wait(Some(self.timing.retry)).await
                }
                Ended::Stopped(why) => {
                    self.set_state(SshState::Ended { why });
                    self.wait(None).await
                }
            };
            if !go_on {
                return;
            }
        }
    }

    async fn try_session(&mut self) -> Result<Ended, Ended> {
        if let Some(problem) = self.config.problem() {
            return Err(Ended::Stopped(problem));
        }
        let stream = self.reach().await?;
        let mut handle = self.handshake(stream).await?;
        self.log_in(&mut handle).await?;
        let ended = self.serve(&handle).await;
        // The other end hears that it is over, if the connection can still say so.
        let bye = handle.disconnect(Disconnect::ByApplication, "", "");
        let _ = timeout(Duration::from_millis(200), bye).await;
        ended
    }

    /// Connects to the server's port.
    async fn reach(&mut self) -> Result<TcpStream, Ended> {
        let host = self.config.host_name();
        let port = self.config.port;
        let lookup = {
            let host = host.clone();
            spawn_blocking(move || addresses(&host, port))
        };
        let found = self.during(lookup).await?;
        let found = found
            .unwrap_or_else(|error| Err(io::Error::other(error)))
            .map_err(|error| Ended::Lost(Some(format!("Cannot find the host {host}: {error}"))))?;

        let mut last = None;
        for address in found {
            let connect = timeout(self.timing.connect, TcpStream::connect(address));
            match self.during(connect).await? {
                Ok(Ok(stream)) => {
                    // Key presses go out at once instead of waiting to fill a packet.
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Ok(Err(error)) => last = Some(error),
                Err(_) => last = Some(io::ErrorKind::TimedOut.into()),
            }
        }
        match last {
            Some(error) if !is_not_there(&error) => Err(Ended::Lost(Some(format!(
                "Could not connect to {}: {error}",
                self.config.address()
            )))),
            _ => Err(Ended::Lost(None)),
        }
    }

    /// Says hello to the server and checks its key, and asks the user about a
    /// key that is not trusted yet.
    async fn handshake(&mut self, stream: TcpStream) -> Result<Handle<Client>, Ended> {
        let host = self.config.host_name();
        let port = self.config.port;
        self.heard = Arc::default();
        let client = Client {
            known_hosts: self.places.known_hosts.clone(),
            host: host.clone(),
            port,
            heard: self.heard.clone(),
        };
        let config = Arc::new(self.settings(&host, port));
        let hello = client::connect_stream(config, stream, client);
        let error = match self.during(timeout(self.timing.handshake, hello)).await? {
            Ok(Ok(handle)) => return Ok(handle),
            Ok(Err(error)) => error,
            Err(_) => {
                return Err(Ended::Lost(Some(format!(
                    "No SSH server answers at {}.",
                    self.config.address()
                ))));
            }
        };

        let (distrusted, problem) = {
            let mut heard = lock(&self.heard);
            (heard.distrusted.take(), heard.problem.take())
        };
        if let Some(problem) = problem {
            return Err(Ended::Stopped(problem));
        }
        let Some((key, verdict)) = distrusted else {
            return Err(self.handshake_failed(error));
        };
        Err(match verdict {
            // The key is checked by the connection itself: `Known` does not end it.
            Verdict::Known => Ended::Reconnect,
            Verdict::Unknown { others } => self.ask_about_new_key(&host, &key, others).await?,
            Verdict::Changed { known } => self.refuse_changed_key(&key, known).await?,
        })
    }

    /// How the connection is made: what `known_hosts` has for this server is
    /// asked for first, and old boards' algorithms come last.
    fn settings(&self, host: &str, port: u16) -> client::Config {
        let usual = Preferred::DEFAULT;
        let key = known_hosts::key_types(&self.places.known_hosts, host, port, &usual.key);
        client::Config {
            client_id: SshId::Standard(format!("SSH-2.0-terNIMA_{}", crate::VERSION).into()),
            keepalive_interval: Some(self.timing.keepalive),
            preferred: Preferred {
                kex: [&usual.kex[..], &OTHER_KEX].concat().into(),
                key: key.into(),
                cipher: [&usual.cipher[..], &OTHER_CIPHERS].concat().into(),
                mac: [&usual.mac[..], &OTHER_MACS].concat().into(),
                ..usual
            },
            ..client::Config::default()
        }
    }

    async fn ask_about_new_key(
        &mut self,
        host: &str,
        key: &PublicKey,
        others: Vec<String>,
    ) -> Result<Ended, Ended> {
        let file = self.places.known_hosts.clone();
        let question = Question::HostKey {
            address: self.config.address(),
            key_type: key.algorithm().as_str().to_string(),
            fingerprint: known_hosts::fingerprint(key),
            others,
            file: file.display().to_string(),
        };
        match self.ask(question).await? {
            Answer::Yes => {
                known_hosts::trust(&file, host, self.config.port, key).map_err(Ended::Stopped)?;
                // The server stopped waiting: connect again, and it is known now.
                Ok(Ended::Reconnect)
            }
            _ => Ok(Ended::Stopped(format!(
                "Did not connect: the key of {} is not trusted.",
                self.config.address()
            ))),
        }
    }

    async fn refuse_changed_key(&mut self, key: &PublicKey, known: String) -> Result<Ended, Ended> {
        let address = self.config.address();
        let file = self.places.known_hosts.display().to_string();
        let name = if self.config.port == 22 {
            self.config.host_name()
        } else {
            format!("[{}]:{}", self.config.host_name(), self.config.port)
        };
        self.ask(Question::KeyChanged {
            address: address.clone(),
            key_type: key.algorithm().as_str().to_string(),
            fingerprint: known_hosts::fingerprint(key),
            known,
            file: file.clone(),
            forget: format!("ssh-keygen -R \"{name}\""),
        })
        .await?;
        Ok(Ended::Stopped(format!(
            "Refused to connect: the key of {address} is not the one in {file}."
        )))
    }

    /// In plain words, why the handshake failed, and whether to try again.
    fn handshake_failed(&self, error: russh::Error) -> Ended {
        let address = self.config.address();
        let closed = lock(&self.heard).goodbye.take();
        match error {
            russh::Error::NoCommonAlgo { kind, theirs, .. } => {
                let what = match kind {
                    AlgorithmKind::Kex => "key exchange",
                    AlgorithmKind::Key => "server key type",
                    AlgorithmKind::Cipher => "cipher",
                    AlgorithmKind::Compression => "compression",
                    AlgorithmKind::Mac => "MAC",
                };
                Ended::Stopped(format!(
                    "{address} and terNIMA have no {what} in common. The server offers: {}.",
                    theirs.join(", ")
                ))
            }
            russh::Error::Version => Ended::Lost(Some(format!(
                "What answers at {address} is not an SSH server."
            ))),
            russh::Error::Disconnect | russh::Error::HUP => Ended::Lost(Some(match closed {
                Some(words) if !words.is_empty() => {
                    format!("{address} closed the connection: {words}")
                }
                _ => format!("{address} closed the connection."),
            })),
            other => Ended::Lost(Some(format!("Could not connect to {address}: {other}"))),
        }
    }

    /// Logs in as the settings say. Asking with no credentials first tells
    /// which ways the server takes, and some boards let anyone in.
    async fn log_in(&mut self, handle: &mut Handle<Client>) -> Result<(), Ended> {
        let user = self.config.user.trim().to_string();
        let first = self.during(handle.authenticate_none(&user)).await?;
        self.show_banner();
        let methods = match first {
            Ok(AuthResult::Success) => return Ok(()),
            Ok(AuthResult::Failure {
                remaining_methods, ..
            }) if !handle.is_closed() => remaining_methods,
            _ => return Err(self.login_lost()),
        };
        let more = match self.config.auth {
            Auth::Password => Some(methods),
            Auth::Key => self.with_key(handle, &user, &methods).await?,
            Auth::Agent => self.with_agent(handle, &user, &methods).await?,
        };
        // What is left for a password, also as a second step after a key.
        match more {
            Some(methods) => self.with_password(handle, &user, methods).await,
            None => Ok(()),
        }
    }

    /// Shows what the server wants read before the login, once.
    fn show_banner(&mut self) {
        let banner = std::mem::take(&mut lock(&self.heard).banner);
        if !banner.is_empty() {
            let lines = banner.replace("\r\n", "\n").replace('\n', "\r\n");
            self.emit(Event::Data(lines.into_bytes()));
        }
    }

    /// Why a login cannot go on, when the connection ended during it.
    fn login_lost(&self) -> Ended {
        let goodbye = lock(&self.heard).goodbye.take();
        Ended::Stopped(match goodbye {
            Some(words) if !words.is_empty() => {
                format!("The server closed the connection during the login: {words}")
            }
            _ => "The server closed the connection during the login.".into(),
        })
    }

    /// Logs in with a password, or with what the server asks itself, such as
    /// PAM's password prompt or a code, until it says yes or the user cancels.
    async fn with_password(
        &mut self,
        handle: &mut Handle<Client>,
        user: &str,
        mut methods: MethodSet,
    ) -> Result<(), Ended> {
        let title = format!("Log in to {}", self.config.target());
        let mut problem = None;
        loop {
            let result = if methods.contains(&MethodKind::Password) {
                let password = self
                    .ask_secret(&title, "Password", false, problem.take(), TO_THE_SERVER)
                    .await?;
                self.during(handle.authenticate_password(user, password))
                    .await?
            } else if methods.contains(&MethodKind::KeyboardInteractive) {
                self.with_server_prompts(handle, user, &title, problem.take())
                    .await?
            } else if problem.is_some() {
                // Some servers take a way off their list after one wrong answer.
                return Err(Ended::Stopped(
                    "The server did not accept that, and allows no more tries.".into(),
                ));
            } else {
                return Err(not_taken(&methods, "a password"));
            };
            match result {
                Ok(AuthResult::Success) => return Ok(()),
                Ok(AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                }) if !handle.is_closed() => {
                    if !partial_success {
                        problem = Some("The server did not accept that.".to_string());
                    }
                    methods = remaining_methods;
                }
                _ => return Err(self.login_lost()),
            }
        }
    }

    /// One round of the server's own questions: keyboard-interactive login.
    async fn with_server_prompts(
        &mut self,
        handle: &mut Handle<Client>,
        user: &str,
        title: &str,
        mut problem: Option<String>,
    ) -> Result<Result<AuthResult, russh::Error>, Ended> {
        let start = handle.authenticate_keyboard_interactive_start(user, None);
        let mut reply = self.during(start).await?;
        loop {
            match reply {
                Err(error) => return Ok(Err(error)),
                Ok(KeyboardInteractiveAuthResponse::Success) => return Ok(Ok(AuthResult::Success)),
                Ok(KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods,
                    partial_success,
                }) => {
                    return Ok(Ok(AuthResult::Failure {
                        remaining_methods,
                        partial_success,
                    }));
                }
                Ok(KeyboardInteractiveAuthResponse::InfoRequest {
                    instructions,
                    prompts,
                    ..
                }) => {
                    let mut answers = Vec::with_capacity(prompts.len());
                    for prompt in prompts {
                        let text = match instructions.trim() {
                            "" => prompt.prompt,
                            instructions => format!("{instructions}\n{}", prompt.prompt),
                        };
                        let answer = self
                            .ask_secret(title, &text, prompt.echo, problem.take(), TO_THE_SERVER)
                            .await?;
                        answers.push(answer);
                    }
                    let respond = handle.authenticate_keyboard_interactive_respond(answers);
                    reply = self.during(respond).await?;
                }
            }
        }
    }

    /// Logs in with the key file. Returns what the server wants next, if the
    /// key was only the first of two steps.
    async fn with_key(
        &mut self,
        handle: &mut Handle<Client>,
        user: &str,
        methods: &MethodSet,
    ) -> Result<Option<MethodSet>, Ended> {
        if !methods.contains(&MethodKind::PublicKey) {
            return Err(not_taken(methods, "a key"));
        }
        let path = self.config.key_path(&self.places.home);
        let shown = path.display().to_string();
        let mut passphrase: Option<String> = None;
        let mut problem = None;
        let key = loop {
            let read = {
                let path = path.clone();
                let passphrase = passphrase.clone();
                spawn_blocking(move || keys::load_secret_key(path, passphrase.as_deref()))
            };
            let error = match self.during(read).await? {
                Ok(Ok(key)) => break key,
                Ok(Err(error)) => error,
                Err(error) => {
                    return Err(Ended::Stopped(format!("Could not read {shown}: {error}")));
                }
            };
            if !needs_passphrase(&error) {
                if passphrase.is_some() && !matches!(error, keys::Error::IO(_)) {
                    problem = Some("That passphrase does not open the key file.".to_string());
                } else {
                    return Err(Ended::Stopped(key_file_problem(&shown, &error)));
                }
            }
            let prompt = format!("Passphrase for {shown}");
            let typed = self
                .ask_secret(
                    "Open the key file",
                    &prompt,
                    false,
                    problem.take(),
                    TO_THE_KEY_FILE,
                )
                .await?;
            passphrase = Some(typed);
        };
        drop(passphrase);

        let hash = self.rsa_hash(handle, &key.algorithm()).await?;
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        let answer = self
            .during(handle.authenticate_publickey(user, key))
            .await?;
        match answer {
            Ok(AuthResult::Success) => Ok(None),
            Ok(AuthResult::Failure {
                remaining_methods,
                partial_success: true,
            }) => Ok(Some(remaining_methods)),
            Ok(AuthResult::Failure { .. }) if !handle.is_closed() => Err(Ended::Stopped(format!(
                "The server did not accept the key {shown} for {user}. Its public half has to be in {user}'s ~/.ssh/authorized_keys there."
            ))),
            _ => Err(self.login_lost()),
        }
    }

    /// Logs in with the keys in the SSH agent, one after another. Returns what
    /// the server wants next, if a key was only the first of two steps.
    async fn with_agent(
        &mut self,
        handle: &mut Handle<Client>,
        user: &str,
        methods: &MethodSet,
    ) -> Result<Option<MethodSet>, Ended> {
        if !methods.contains(&MethodKind::PublicKey) {
            return Err(not_taken(methods, "a key"));
        }
        let place = self.places.agent.clone();
        let mut agent = self
            .during(connect_agent(place))
            .await?
            .map_err(|()| Ended::Stopped(NO_AGENT.into()))?;
        // Asking is also how Pageant's connection finds out if Pageant is there.
        let identities = self
            .during(agent.request_identities())
            .await?
            .map_err(|_| Ended::Stopped(NO_AGENT.into()))?;
        let keys: Vec<PublicKey> = identities
            .into_iter()
            .filter_map(|identity| match identity {
                AgentIdentity::PublicKey { key, .. } => Some(key),
                AgentIdentity::Certificate { .. } => None,
            })
            .collect();
        if keys.is_empty() {
            return Err(Ended::Stopped(
                "The SSH agent holds no keys. Add one with ssh-add.".into(),
            ));
        }

        let tried = keys.len();
        let mut refused = None;
        for key in keys {
            let hash = self.rsa_hash(handle, &key.algorithm()).await?;
            let answer = self
                .during(handle.authenticate_publickey_with(user, key, hash, &mut agent))
                .await?;
            match answer {
                Ok(AuthResult::Success) => return Ok(None),
                Ok(AuthResult::Failure {
                    remaining_methods,
                    partial_success: true,
                }) => {
                    return Ok(Some(remaining_methods));
                }
                Ok(AuthResult::Failure { .. }) if !handle.is_closed() => {}
                Ok(AuthResult::Failure { .. }) => return Err(self.login_lost()),
                // Such as a key the agent will not use without a confirmation.
                Err(error) => refused = Some(error.to_string()),
            }
        }
        Err(Ended::Stopped(match refused {
            Some(error) => format!("The SSH agent did not sign with its key: {error}"),
            None => {
                format!("The server did not accept the {tried} key(s) in the SSH agent for {user}.")
            }
        }))
    }

    /// How to sign with a key: an RSA key the way the server likes best.
    async fn rsa_hash(
        &mut self,
        handle: &Handle<Client>,
        algorithm: &Algorithm,
    ) -> Result<Option<HashAlg>, Ended> {
        if !matches!(algorithm, Algorithm::Rsa { .. }) {
            return Ok(None);
        }
        let best = self.during(handle.best_supported_rsa_hash()).await?;
        Ok(best.ok().flatten().flatten())
    }

    /// Opens the shell in a terminal, then passes data both ways until the
    /// session closes, the connection ends or the settings change.
    async fn serve(&mut self, handle: &Handle<Client>) -> Result<Ended, Ended> {
        let opened = self.during(handle.channel_open_session()).await?;
        let channel = opened.map_err(|error| match error {
            russh::Error::ChannelOpenFailure(_) => {
                Ended::Stopped("The server did not open a session for this user.".into())
            }
            _ => Ended::Lost(None),
        })?;
        let (cols, rows) = self.size;
        let asked = channel
            .request_pty(true, TERM, cols, rows, 0, 0, &TERMINAL_MODES)
            .await
            .and(channel.request_shell(true).await);
        if asked.is_err() {
            return Err(Ended::Lost(None));
        }
        let (mut reader, writer) = channel.split();
        let mut pending = Vec::new();
        for what in ["a terminal", "a shell"] {
            match self.during(reply(&mut reader, &mut pending)).await? {
                Some(true) => {}
                Some(false) => {
                    return Err(Ended::Stopped(format!(
                        "The server did not give {what}. It may only allow file transfers for this user."
                    )));
                }
                None => return Err(Ended::Lost(None)),
            }
        }
        self.set_state(SshState::Connected {
            address: self.config.target(),
        });

        let (outgoing, queue) = mpsc::unbounded_channel();
        let sending = tokio::spawn(send_out(writer, queue));
        let ended = self.pass_data(&mut reader, &outgoing, pending).await;
        sending.abort();
        Ok(ended)
    }

    async fn pass_data(
        &mut self,
        reader: &mut ChannelReadHalf,
        outgoing: &UnboundedSender<Out>,
        mut pending: Vec<u8>,
    ) -> Ended {
        let mut due = (!pending.is_empty()).then(|| Instant::now() + self.timing.batch);
        let mut status = None;
        let ended = loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    None | Some(Command::Close) => break Ended::Closed,
                    Some(Command::Reconfigure(config)) => {
                        self.config = config;
                        break Ended::Reconnect;
                    }
                    Some(Command::Write(data)) => {
                        let _ = outgoing.send(Out::Data(data));
                    }
                    Some(Command::Resize { cols, rows }) => {
                        if self.size != (cols, rows) {
                            self.size = (cols, rows);
                            let _ = outgoing.send(Out::Size(cols, rows));
                        }
                    }
                    Some(Command::Answer(_)) => {}
                },
                message = reader.wait() => match message {
                    Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                        pending.extend_from_slice(&data);
                        due.get_or_insert_with(|| Instant::now() + self.timing.batch);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => status = Some(exit_status),
                    Some(ChannelMsg::Close) | None => {
                        break match status {
                            Some(0) => Ended::Stopped("The shell ended.".into()),
                            Some(status) => Ended::Stopped(format!("The shell ended, with status {status}.")),
                            // Killed, or the connection broke, as when the board restarts.
                            None => Ended::Lost(None),
                        };
                    }
                    Some(_) => {}
                },
                () = sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                    self.emit(Event::Data(std::mem::take(&mut pending)));
                    due = None;
                }
            }
            if pending.len() >= BATCH_MAX {
                self.emit(Event::Data(std::mem::take(&mut pending)));
                due = None;
            }
        };
        if !pending.is_empty() {
            self.emit(Event::Data(pending));
        }
        ended
    }

    /// Waits for `work`, still doing what the app asks. Closing the session
    /// or new settings end the wait.
    async fn during<T>(&mut self, work: impl Future<Output = T>) -> Result<T, Ended> {
        tokio::pin!(work);
        loop {
            tokio::select! {
                output = &mut work => return Ok(output),
                command = self.commands.recv() => {
                    self.idle(command)?;
                }
            }
        }
    }

    /// Does what the app asks while no shell is open, and gives back an answer if it is one.
    fn idle(&mut self, command: Option<Command>) -> Result<Option<Answer>, Ended> {
        match command {
            None | Some(Command::Close) => Err(Ended::Closed),
            Some(Command::Reconfigure(config)) => {
                self.config = config;
                Err(Ended::Reconnect)
            }
            Some(Command::Resize { cols, rows }) => {
                self.size = (cols, rows);
                Ok(None)
            }
            // Nothing to send it to.
            Some(Command::Write(_)) => Ok(None),
            Some(Command::Answer(answer)) => Ok(Some(answer)),
        }
    }

    /// Asks the user, and waits as long as it takes.
    async fn ask(&mut self, question: Question) -> Result<Answer, Ended> {
        self.emit(Event::Ask(question));
        loop {
            let command = self.commands.recv().await;
            if let Some(answer) = self.idle(command)? {
                return Ok(answer);
            }
        }
    }

    /// Asks for something to type. Cancelling ends the session.
    async fn ask_secret(
        &mut self,
        title: &str,
        prompt: &str,
        echo: bool,
        problem: Option<String>,
        note: &str,
    ) -> Result<String, Ended> {
        let question = Question::Secret {
            title: title.to_string(),
            prompt: prompt.to_string(),
            echo,
            problem,
            note: note.to_string(),
        };
        match self.ask(question).await? {
            Answer::Text { text } => Ok(text),
            _ => Err(Ended::Stopped("The login was cancelled.".into())),
        }
    }

    /// Waits while not connected, still doing what the app asks: for `time`,
    /// or until told otherwise. Returns false when the session should end.
    async fn wait(&mut self, time: Option<Duration>) -> bool {
        let waited = self
            .during(async {
                match time {
                    Some(time) => sleep(time).await,
                    None => pending().await,
                }
            })
            .await;
        // New settings try again at once.
        matches!(waited, Ok(()) | Err(Ended::Reconnect))
    }

    fn set_state(&mut self, state: SshState) {
        if self.state.as_ref() != Some(&state) {
            self.state = Some(state.clone());
            self.emit(Event::State(state));
        }
    }

    fn emit(&mut self, event: Event) {
        (self.on_event)(event);
    }
}

/// Waits for the server's yes or no to a request, keeping what the shell says
/// meanwhile. `None` when the channel ended first.
async fn reply(reader: &mut ChannelReadHalf, early: &mut Vec<u8>) -> Option<bool> {
    while let Some(message) = reader.wait().await {
        match message {
            ChannelMsg::Success => return Some(true),
            ChannelMsg::Failure => return Some(false),
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                early.extend_from_slice(&data);
            }
            ChannelMsg::Close => return None,
            _ => {}
        }
    }
    None
}

/// The session's end when the server takes no such way to log in, in words.
fn not_taken(methods: &MethodSet, way: &str) -> Ended {
    let names: Vec<&str> = methods.iter().map(<&str>::from).collect();
    Ended::Stopped(format!(
        "This server does not take {way} for this user. It takes: {}.",
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    ))
}

/// Whether a key file could not be read only for want of its passphrase.
fn needs_passphrase(error: &keys::Error) -> bool {
    // By its words, as a PuTTY key's error has a type that cannot be named from here.
    error.to_string().contains("is encrypted")
}

/// Why a key file cannot be used, in words.
fn key_file_problem(shown: &str, error: &keys::Error) -> String {
    match error {
        keys::Error::IO(error) => format!("Could not read the key file {shown}: {error}"),
        _ if shown.ends_with(".pub") => {
            format!("{shown} is the public half of a key. Choose the file without .pub.")
        }
        error => format!("{shown} is not a private key terNIMA can read: {error}"),
    }
}

/// A connection to the SSH agent, whichever kind it is.
type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// Connects to the agent `place` names, or else OpenSSH's named pipe, and then Pageant.
#[cfg(windows)]
async fn connect_agent(place: Option<OsString>) -> Result<Agent, ()> {
    // Only a pipe can be reached here: MSYS's and Cygwin's agents give a path of their own.
    let pipe = place
        .filter(|place| place.to_string_lossy().starts_with(r"\\.\pipe\"))
        .unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".into());
    if let Ok(agent) = AgentClient::connect_named_pipe(&pipe).await {
        return Ok(agent.dynamic());
    }
    match AgentClient::connect_pageant().await {
        Ok(agent) => Ok(agent.dynamic()),
        Err(_) => Err(()),
    }
}

/// Connects to the agent `place` names: `SSH_AUTH_SOCK`.
#[cfg(not(windows))]
async fn connect_agent(place: Option<OsString>) -> Result<Agent, ()> {
    let place = place.ok_or(())?;
    match AgentClient::connect_uds(&place).await {
        Ok(agent) => Ok(agent.dynamic()),
        Err(_) => Err(()),
    }
}

#[cfg(test)]
mod tests;
