//! The session against a real SSH server: russh's own, on this machine.

use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self as std_mpsc, Receiver};
use std::time::Instant as Clock;

use russh::keys::ssh_key::{Cipher, Kdf, LineEnding};
use russh::keys::{EcdsaCurve, PrivateKey};
use russh::server::{self, Auth as Entry, ChannelOpenHandle, Msg, Response, VerifiedPublicKey};
use russh::{Channel, ChannelId};
use tokio::task::JoinSet;

use super::*;
use crate::tomlfile::tests::TempDir;

const FAST: Timing = Timing {
    retry: Duration::from_millis(30),
    connect: Duration::from_millis(500),
    handshake: Duration::from_secs(10),
    batch: Duration::from_millis(5),
    keepalive: Duration::from_secs(60),
};

const PASSWORD: &str = "open sesame";

fn new_key(algorithm: Algorithm) -> PrivateKey {
    PrivateKey::random(&mut keys::key::safe_rng(), algorithm).unwrap()
}

/// The ways into a test server.
#[derive(Clone, Default)]
struct Door {
    password: Option<&'static str>,
    /// The answer to the server's own question, the way PAM asks for a password.
    word: Option<&'static str>,
    key: Option<PublicKey>,
    /// Anyone gets in, as on some boards.
    open: bool,
    banner: Option<&'static str>,
    /// Gives no terminal, as for a user who may only transfer files.
    no_terminal: bool,
    /// A wrong password closes this way in.
    one_try: bool,
}

fn password_door() -> Door {
    Door {
        password: Some(PASSWORD),
        ..Door::default()
    }
}

fn key_door(key: &PrivateKey) -> Door {
    Door {
        key: Some(key.public_key().clone()),
        ..Door::default()
    }
}

impl Door {
    /// The ways in that the server offers.
    fn methods(&self) -> MethodSet {
        let mut methods = MethodSet::empty();
        if self.password.is_some() {
            methods.push(MethodKind::Password);
        }
        if self.word.is_some() {
            methods.push(MethodKind::KeyboardInteractive);
        }
        if self.key.is_some() {
            methods.push(MethodKind::PublicKey);
        }
        methods
    }

    /// A wrong answer: the ways in stay open, as OpenSSH leaves them, unless
    /// the door has one try. russh's server by itself takes a way off its list.
    fn refuse(&self) -> Entry {
        Entry::Reject {
            proceed_with_methods: (!self.one_try).then(|| self.methods()),
            partial_success: false,
        }
    }
}

/// What a test server saw.
#[derive(Default)]
struct Seen {
    connections: usize,
    passwords: Vec<String>,
    terminal: Option<(String, u32, u32)>,
    modes: Vec<(Pty, u32)>,
    sizes: Vec<(u32, u32)>,
    shell: bool,
}

type SharedSeen = Arc<Mutex<Seen>>;

fn seen(seen: &SharedSeen) -> MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A connection to a test server: its shell echoes what it gets, and "exit"
/// or "exit 3" ends it with that status.
struct Visit {
    door: Door,
    seen: SharedSeen,
}

impl server::Handler for Visit {
    type Error = russh::Error;

    async fn authentication_banner(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.door.banner.map(str::to_string))
    }

    async fn auth_none(&mut self, _user: &str) -> Result<Entry, Self::Error> {
        Ok(if self.door.open {
            Entry::Accept
        } else {
            Entry::reject()
        })
    }

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Entry, Self::Error> {
        seen(&self.seen).passwords.push(password.to_string());
        Ok(if self.door.password == Some(password) {
            Entry::Accept
        } else {
            self.door.refuse()
        })
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Entry, Self::Error> {
        // Not the keys themselves: those compare their comments too.
        let right = self.door.key.as_ref().map(PublicKey::key_data);
        Ok(if right == Some(key.public_key.key_data()) {
            Entry::Accept
        } else {
            self.door.refuse()
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Entry, Self::Error> {
        let Some(mut answers) = response else {
            return Ok(Entry::Partial {
                name: "".into(),
                instructions: "Say the word.".into(),
                prompts: vec![("Word: ".into(), false)].into(),
            });
        };
        let right = self.door.word.map(str::as_bytes);
        Ok(if answers.next().as_deref() == right {
            Entry::Accept
        } else {
            self.door.refuse()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        modes: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.door.no_terminal {
            return session.channel_failure(channel);
        }
        let mut seen = seen(&self.seen);
        seen.terminal = Some((term.to_string(), cols, rows));
        seen.modes = modes.to_vec();
        session.channel_success(channel)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        seen(&self.seen).shell = true;
        session.channel_success(channel)?;
        session.data(channel, b"$ ".to_vec())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        cols: u32,
        rows: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        seen(&self.seen).sizes.push((cols, rows));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if let Some(status) = String::from_utf8_lossy(data).strip_prefix("exit") {
            session.exit_status_request(channel, status.trim().parse().unwrap_or(0))?;
            session.eof(channel)?;
            return session.close(channel);
        }
        session.data(channel, data.to_vec())
    }
}

/// An SSH server on a port of this machine, on a thread of its own.
struct TestServer {
    port: u16,
    key: PublicKey,
    seen: SharedSeen,
    /// Breaks every connection when something is sent; dropped, it ends the server.
    kick: UnboundedSender<()>,
}

impl TestServer {
    fn new(door: Door) -> Self {
        Self::on_port(door, 0)
    }

    fn on_port(door: Door, port: u16) -> Self {
        let host_key = new_key(Algorithm::Ed25519);
        let key = host_key.public_key().clone();
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(server::Config {
            methods: door.methods(),
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::from_millis(1)),
            keys: vec![host_key],
            // A small window: russh does not read while it writes, so with both
            // ends russh, and more in flight than the sockets hold, both would
            // wait for the other. OpenSSH always reads.
            window_size: 64 * 1024,
            ..server::Config::default()
        });
        let shared = SharedSeen::default();
        let (kick, mut kicks) = mpsc::unbounded_channel();
        let seen_there = shared.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = TcpListener::from_std(listener).unwrap();
                let mut visits = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut stream, _) = accepted.unwrap();
                            seen(&seen_there).connections += 1;
                            let visit = Visit { door: door.clone(), seen: seen_there.clone() };
                            let config = config.clone();
                            // Between the socket and the server, so a kick can close the socket.
                            let (mut near, far) = tokio::io::duplex(1024 * 1024);
                            tokio::spawn(server::run_stream(config, far, visit));
                            visits.spawn(async move {
                                let _ = tokio::io::copy_bidirectional(&mut stream, &mut near).await;
                            });
                        }
                        kicked = kicks.recv() => match kicked {
                            Some(()) => visits.abort_all(),
                            None => return,
                        },
                    }
                }
            });
            runtime.shutdown_background();
        });
        Self {
            port,
            key,
            seen: shared,
            kick,
        }
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        seen(&self.seen)
    }

    fn config(&self, auth: Auth) -> SshConfig {
        SshConfig {
            host: "127.0.0.1".into(),
            port: self.port,
            user: "root".into(),
            auth,
            key_file: String::new(),
        }
    }
}

use tokio::net::TcpListener;

/// A place for one test: its `known_hosts`, its key files, and where its agent is.
struct Bench {
    dir: TempDir,
    agent: Option<OsString>,
}

impl Bench {
    fn new(name: &str) -> Self {
        Self {
            dir: TempDir::new(&format!("ssh-{name}")),
            agent: None,
        }
    }

    fn known_hosts(&self) -> PathBuf {
        self.dir.file("known_hosts")
    }

    /// Puts a server's key in `known_hosts`, as if it was trusted before.
    fn trust(&self, server: &TestServer) {
        known_hosts::trust(&self.known_hosts(), "127.0.0.1", server.port, &server.key).unwrap();
    }

    /// Writes a private key to a file, and gives its path.
    fn key_file(&self, key: &PrivateKey) -> String {
        let file = self.dir.file("id_ed25519");
        fs::write(&file, key.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        file.display().to_string()
    }

    /// Starts a session and gives back what it reports.
    fn start(&self, config: SshConfig) -> (Session, Receiver<Event>) {
        self.start_timed(config, FAST)
    }

    fn start_timed(&self, config: SshConfig, timing: Timing) -> (Session, Receiver<Event>) {
        let (sender, events) = std_mpsc::channel();
        let places = Places {
            home: self.dir.file(""),
            known_hosts: self.known_hosts(),
            agent: self.agent.clone(),
        };
        let session = Session::start_with(
            config,
            (100, 30),
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
            timing,
            places,
        );
        (session, events)
    }
}

/// Waits for the first event that `wanted` accepts, skipping the others.
fn expect(events: &Receiver<Event>, what: &str, wanted: impl Fn(&Event) -> bool) -> Event {
    // A PuTTY key's passphrase takes seconds to try in a debug build.
    let until = Clock::now() + Duration::from_secs(60);
    while let Some(left) = until.checked_duration_since(Clock::now()) {
        match events.recv_timeout(left) {
            Ok(event) if wanted(&event) => return event,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("no {what} within 60 s");
}

fn connected(events: &Receiver<Event>) -> String {
    match expect(events, "connect", |event| {
        matches!(event, Event::State(SshState::Connected { .. }))
    }) {
        Event::State(SshState::Connected { address }) => address,
        _ => unreachable!(),
    }
}

fn waiting(events: &Receiver<Event>) -> Option<String> {
    match expect(events, "wait", |event| {
        matches!(event, Event::State(SshState::Waiting { .. }))
    }) {
        Event::State(SshState::Waiting { problem }) => problem,
        _ => unreachable!(),
    }
}

fn ended(events: &Receiver<Event>) -> String {
    match expect(events, "end", |event| {
        matches!(event, Event::State(SshState::Ended { .. }))
    }) {
        Event::State(SshState::Ended { why }) => why,
        _ => unreachable!(),
    }
}

/// Waits for the session to ask something, whatever it is.
fn asked(events: &Receiver<Event>) -> Question {
    match expect(events, "question", |event| matches!(event, Event::Ask(_))) {
        Event::Ask(question) => question,
        _ => unreachable!(),
    }
}

/// Waits for the session to ask for something to type: its prompt, and why it asks again.
fn asked_secret(events: &Receiver<Event>) -> (String, Option<String>) {
    match asked(events) {
        Question::Secret {
            prompt, problem, ..
        } => (prompt, problem),
        question => panic!("{question:?}"),
    }
}

fn answer_text(session: &Session, text: &str) {
    session.answer(Answer::Text { text: text.into() });
}

/// Logs in with the password, for the tests that are about something else.
fn log_in(session: &Session, events: &Receiver<Event>) {
    asked_secret(events);
    answer_text(session, PASSWORD);
    connected(events);
}

/// Collects received data until it has `text`.
fn received(events: &Receiver<Event>, text: &str) -> String {
    let mut data = Vec::new();
    while !String::from_utf8_lossy(&data).contains(text) {
        if let Event::Data(bytes) = expect(events, "data", |event| matches!(event, Event::Data(_)))
        {
            data.extend(bytes);
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

/// Waits for something that happens on the server's side.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let until = Clock::now() + Duration::from_secs(10);
    while Clock::now() < until {
        if done() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{what} did not happen within 10 s");
}

/// Gives the session time to do something it should not, such as trying again.
fn a_few_retries() {
    thread::sleep(FAST.retry * 5);
}

#[test]
fn logs_in_with_a_password_and_passes_data_both_ways() {
    let bench = Bench::new("password");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));

    let (prompt, problem) = asked_secret(&events);
    assert_eq!(prompt, "Password");
    assert_eq!(problem, None);
    answer_text(&session, PASSWORD);
    assert_eq!(
        connected(&events),
        format!("root@127.0.0.1:{}", server.port)
    );
    received(&events, "$ ");

    session.write(b"help\r".to_vec());
    received(&events, "help\r");
    assert!(server.seen().shell);
}

#[test]
fn a_real_terminal_is_asked_for_at_the_size_it_has() {
    let bench = Bench::new("terminal");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    asked_secret(&events);
    // The tab changed size while its user was typing the password.
    session.resize(90, 20);
    answer_text(&session, PASSWORD);
    connected(&events);

    let seen = server.seen();
    assert_eq!(seen.terminal, Some(("xterm-256color".into(), 90, 20)));
    assert!(seen.modes.contains(&(Pty::VERASE, 127)), "{:?}", seen.modes);
}

#[test]
fn a_new_size_is_told_once() {
    let bench = Bench::new("resize");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);

    session.resize(120, 40);
    session.resize(120, 40);
    session.resize(80, 24);
    eventually("the new sizes", || server.seen().sizes.len() >= 2);
    assert_eq!(server.seen().sizes, [(120, 40), (80, 24)]);
}

#[test]
fn a_new_servers_key_is_asked_about_first_and_saved_when_trusted() {
    let bench = Bench::new("new-key");
    let server = TestServer::new(password_door());
    let (session, events) = bench.start(server.config(Auth::Password));

    let Question::HostKey {
        address,
        key_type,
        fingerprint,
        others,
        file,
    } = asked(&events)
    else {
        panic!("another question");
    };
    assert_eq!(address, format!("127.0.0.1:{}", server.port));
    assert_eq!(key_type, "ssh-ed25519");
    assert_eq!(fingerprint, known_hosts::fingerprint(&server.key));
    assert!(others.is_empty());
    assert_eq!(file, bench.known_hosts().display().to_string());
    // Nothing is saved and nobody logs in until the user says so.
    assert!(!bench.known_hosts().exists());
    assert!(server.seen().passwords.is_empty());

    session.answer(Answer::Yes);
    log_in(&session, &events);
    let saved = fs::read_to_string(bench.known_hosts()).unwrap();
    assert!(
        saved.contains(&format!("[127.0.0.1]:{} ssh-ed25519 ", server.port)),
        "{saved}"
    );
}

#[test]
fn a_new_servers_key_not_trusted_ends_the_session_and_is_not_saved() {
    let bench = Bench::new("new-key-no");
    let server = TestServer::new(password_door());
    let (session, events) = bench.start(server.config(Auth::Password));
    assert!(matches!(asked(&events), Question::HostKey { .. }));

    session.answer(Answer::No);
    assert!(ended(&events).contains("not trusted"));
    assert!(!bench.known_hosts().exists());
    a_few_retries();
    assert_eq!(server.seen().connections, 1);
}

#[test]
fn a_changed_key_is_refused_without_logging_in() {
    let bench = Bench::new("changed-key");
    let server = TestServer::new(password_door());
    let old = new_key(Algorithm::Ed25519).public_key().clone();
    known_hosts::trust(&bench.known_hosts(), "127.0.0.1", server.port, &old).unwrap();
    let before = fs::read_to_string(bench.known_hosts()).unwrap();
    let (session, events) = bench.start(server.config(Auth::Password));

    let Question::KeyChanged {
        fingerprint,
        known,
        forget,
        file,
        ..
    } = asked(&events)
    else {
        panic!("another question");
    };
    assert_eq!(fingerprint, known_hosts::fingerprint(&server.key));
    assert_eq!(known, known_hosts::fingerprint(&old));
    assert_eq!(
        forget,
        format!("ssh-keygen -R \"[127.0.0.1]:{}\"", server.port)
    );
    assert_eq!(file, bench.known_hosts().display().to_string());

    session.answer(Answer::No);
    assert!(ended(&events).contains("Refused"));
    a_few_retries();
    // The file is as it was, and the server heard nothing.
    assert_eq!(fs::read_to_string(bench.known_hosts()).unwrap(), before);
    let seen = server.seen();
    assert!(seen.passwords.is_empty());
    assert_eq!(seen.connections, 1);
}

#[test]
fn a_key_of_a_new_type_is_asked_about_and_says_what_was_known() {
    let bench = Bench::new("other-type");
    let server = TestServer::new(password_door());
    let other = new_key(Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    });
    known_hosts::trust(
        &bench.known_hosts(),
        "127.0.0.1",
        server.port,
        other.public_key(),
    )
    .unwrap();
    let (_session, events) = bench.start(server.config(Auth::Password));

    let Question::HostKey {
        key_type, others, ..
    } = asked(&events)
    else {
        panic!("another question");
    };
    assert_eq!(key_type, "ssh-ed25519");
    assert_eq!(others, ["ecdsa-sha2-nistp256"]);
}

#[test]
fn a_wrong_password_is_asked_again_and_cancelling_ends_the_session() {
    let bench = Bench::new("wrong-password");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));

    asked_secret(&events);
    answer_text(&session, "wrong");
    let (_, problem) = asked_secret(&events);
    assert!(problem.is_some());
    answer_text(&session, PASSWORD);
    connected(&events);
    assert_eq!(server.seen().passwords, ["wrong", PASSWORD]);

    // The next login is cancelled.
    let (session, events) = bench.start(server.config(Auth::Password));
    asked_secret(&events);
    session.answer(Answer::No);
    assert!(ended(&events).contains("cancelled"));
}

#[test]
fn a_wrong_password_on_a_server_that_allows_one_try_says_so() {
    let bench = Bench::new("one-try");
    let server = TestServer::new(Door {
        one_try: true,
        ..password_door()
    });
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    asked_secret(&events);
    answer_text(&session, "wrong");
    let why = ended(&events);
    assert!(
        why.contains("during the login") || why.contains("no more tries"),
        "{why}"
    );
    a_few_retries();
    assert_eq!(server.seen().connections, 1);
}

#[test]
fn a_server_that_asks_itself_gets_the_answers_typed() {
    let bench = Bench::new("keyboard");
    let server = TestServer::new(Door {
        word: Some("swordfish"),
        ..Door::default()
    });
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));

    let (prompt, _) = asked_secret(&events);
    assert_eq!(prompt, "Say the word.\nWord: ");
    answer_text(&session, "wrong");
    let (_, problem) = asked_secret(&events);
    assert!(problem.is_some());
    answer_text(&session, "swordfish");
    connected(&events);
}

#[test]
fn a_server_that_lets_anyone_in_asks_nothing() {
    let bench = Bench::new("open");
    let server = TestServer::new(Door {
        open: true,
        ..Door::default()
    });
    bench.trust(&server);
    let (_session, events) = bench.start(server.config(Auth::Password));
    connected(&events);
}

#[test]
fn what_the_server_shows_first_is_passed_on_with_whole_line_ends() {
    let bench = Bench::new("banner");
    let server = TestServer::new(Door {
        banner: Some("This is the lab's board.\nBe nice.\n"),
        ..password_door()
    });
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    received(&events, "This is the lab's board.\r\nBe nice.\r\n");
    log_in(&session, &events);
}

#[test]
fn logs_in_with_a_key_file() {
    let bench = Bench::new("key-file");
    let key = new_key(Algorithm::Ed25519);
    let server = TestServer::new(key_door(&key));
    bench.trust(&server);
    let config = SshConfig {
        key_file: bench.key_file(&key),
        ..server.config(Auth::Key)
    };
    let (_session, events) = bench.start(config);
    connected(&events);
}

#[test]
fn a_key_files_passphrase_is_asked_for_until_it_opens() {
    let bench = Bench::new("passphrase");
    let key = new_key(Algorithm::Ed25519);
    // One round, as the usual sixteen take seconds in a debug build.
    let kdf = Kdf::Bcrypt {
        salt: vec![7; 16],
        rounds: 1,
    };
    let locked = key
        .encrypt_with(Cipher::Aes256Ctr, kdf, 1, "phrase")
        .unwrap();
    let server = TestServer::new(key_door(&key));
    bench.trust(&server);
    let config = SshConfig {
        key_file: bench.key_file(&locked),
        ..server.config(Auth::Key)
    };
    let (session, events) = bench.start(config);

    let (prompt, problem) = asked_secret(&events);
    assert!(prompt.contains("id_ed25519"), "{prompt}");
    assert_eq!(problem, None);
    answer_text(&session, "wrong");
    let (_, problem) = asked_secret(&events);
    assert!(problem.unwrap().contains("passphrase"));
    answer_text(&session, "phrase");
    connected(&events);
}

/// A key made by PuTTYgen, from the ssh-key crate's tests.
const PUTTY_KEY: &str = "PuTTY-User-Key-File-3: ssh-ed25519
Encryption: none
Comment: user@example.com
Public-Lines: 2
AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XF
Sqti
Private-Lines: 1
AAAAILYGwiLRDBba4WxwpNRRc0cuxhfgXGVpINJuVsCPtZHt
Private-MAC: 94140d0344fad6aa1bf7b71e9c93db11ccac8a232f8a51e11c024869d608c82d
";

/// The same key with "123" as its passphrase.
const PUTTY_KEY_LOCKED: &str = "PuTTY-User-Key-File-3: ssh-ed25519
Encryption: aes256-cbc
Comment: user@example.com
Public-Lines: 2
AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XF
Sqti
Key-Derivation: Argon2id
Argon2-Memory: 8192
Argon2-Passes: 34
Argon2-Parallelism: 1
Argon2-Salt: 63d1d43f7bf7700720496646a2f5ec17
Private-Lines: 1
DyWtExZ3dxFutnb12tIwXBC6kWdozrvP+r6faHKBGDb4+qEar9XBiC0BmGySMHUi
Private-MAC: 52fd00d4ef47ebc506e4e709486c0c6bc0606e24fe2c6cb1b3d168f4da238a66
";

#[test]
fn logs_in_with_a_putty_key_also_with_a_passphrase() {
    let bench = Bench::new("putty");
    let file = bench.dir.file("id_ed25519.ppk");

    fs::write(&file, PUTTY_KEY).unwrap();
    let key = keys::load_secret_key(&file, None).unwrap();
    let server = TestServer::new(key_door(&key));
    bench.trust(&server);
    let config = SshConfig {
        key_file: file.display().to_string(),
        ..server.config(Auth::Key)
    };
    let (_session, events) = bench.start(config.clone());
    connected(&events);

    fs::write(&file, PUTTY_KEY_LOCKED).unwrap();
    let (session, events) = bench.start(config);
    let (prompt, _) = asked_secret(&events);
    assert!(prompt.contains("Passphrase"), "{prompt}");
    answer_text(&session, "wrong");
    let (_, problem) = asked_secret(&events);
    assert!(problem.unwrap().contains("passphrase"));
    answer_text(&session, "123");
    connected(&events);
}

#[test]
fn a_key_the_server_does_not_take_ends_the_session_and_is_not_tried_again() {
    let bench = Bench::new("key-refused");
    let server = TestServer::new(key_door(&new_key(Algorithm::Ed25519)));
    bench.trust(&server);
    let config = SshConfig {
        key_file: bench.key_file(&new_key(Algorithm::Ed25519)),
        ..server.config(Auth::Key)
    };
    let (_session, events) = bench.start(config);

    assert!(ended(&events).contains("did not accept the key"));
    a_few_retries();
    assert_eq!(server.seen().connections, 1);
}

#[test]
fn a_key_file_that_cannot_be_used_says_why() {
    let bench = Bench::new("key-bad");
    let server = TestServer::new(key_door(&new_key(Algorithm::Ed25519)));
    bench.trust(&server);
    let with_file = |file: PathBuf| SshConfig {
        key_file: file.display().to_string(),
        ..server.config(Auth::Key)
    };

    let (_session, events) = bench.start(with_file(bench.dir.file("missing")));
    assert!(ended(&events).contains("Could not read the key file"));

    let public = bench.dir.file("id_ed25519.pub");
    let line = new_key(Algorithm::Ed25519)
        .public_key()
        .to_openssh()
        .unwrap();
    fs::write(&public, line).unwrap();
    let (_session, events) = bench.start(with_file(public));
    assert!(ended(&events).contains("public half"));

    let text = bench.dir.file("notes.txt");
    fs::write(&text, "not a key").unwrap();
    let (_session, events) = bench.start(with_file(text));
    assert!(ended(&events).contains("not a private key"));
}

#[test]
fn a_server_without_the_way_chosen_says_which_it_has() {
    let bench = Bench::new("other-way");
    let keys_only = TestServer::new(key_door(&new_key(Algorithm::Ed25519)));
    bench.trust(&keys_only);
    let (_session, events) = bench.start(keys_only.config(Auth::Password));
    let why = ended(&events);
    assert!(
        why.contains("a password") && why.contains("publickey"),
        "{why}"
    );

    let passwords_only = TestServer::new(password_door());
    bench.trust(&passwords_only);
    let config = SshConfig {
        key_file: "id_ed25519".into(),
        ..passwords_only.config(Auth::Key)
    };
    let (_session, events) = bench.start(config);
    let why = ended(&events);
    assert!(why.contains("a key") && why.contains("password"), "{why}");
}

/// The name of an agent no other test uses.
fn agent_place(bench: &Bench) -> OsString {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    if cfg!(windows) {
        format!(
            r"\\.\pipe\ternima-test-agent-{}-{number}",
            std::process::id()
        )
        .into()
    } else {
        bench.dir.file(&format!("agent-{number}")).into_os_string()
    }
}

#[derive(Clone)]
struct TestAgent;

impl keys::agent::server::Agent for TestAgent {}

/// Connections to an agent at `place`, for russh's agent server.
#[cfg(windows)]
fn agent_connections(
    place: OsString,
) -> impl futures::Stream<Item = io::Result<tokio::net::windows::named_pipe::NamedPipeServer>> + Unpin
{
    use tokio::net::windows::named_pipe::ServerOptions;
    let first = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&place)
        .unwrap();
    Box::pin(futures::stream::unfold(first, move |server| {
        let place = place.clone();
        async move {
            let connected = server.connect().await.map(|()| server);
            let next = ServerOptions::new().create(&place).ok()?;
            Some((connected, next))
        }
    }))
}

#[cfg(not(windows))]
fn agent_connections(
    place: OsString,
) -> impl futures::Stream<Item = io::Result<tokio::net::UnixStream>> + Unpin {
    let listener = tokio::net::UnixListener::bind(place).unwrap();
    Box::pin(futures::stream::unfold(listener, |listener| async move {
        let accepted = listener.accept().await.map(|(stream, _)| stream);
        Some((accepted, listener))
    }))
}

/// Starts an SSH agent at `place`, holding `keys`, on a thread of its own.
fn start_agent(place: OsString, keys: Vec<PrivateKey>) {
    let (ready, is_ready) = std_mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let connections = agent_connections(place.clone());
            tokio::spawn(keys::agent::server::serve(connections, TestAgent));
            let mut agent = connect_agent(Some(place)).await.unwrap();
            for key in keys {
                agent.add_identity(&key, &[]).await.unwrap();
            }
            ready.send(()).unwrap();
            pending::<()>().await;
        });
    });
    is_ready.recv().unwrap();
}

#[test]
fn logs_in_with_the_key_the_agent_holds_that_the_server_takes() {
    let mut bench = Bench::new("agent");
    let key = new_key(Algorithm::Ed25519);
    let place = agent_place(&bench);
    start_agent(
        place.clone(),
        vec![new_key(Algorithm::Ed25519), key.clone()],
    );
    bench.agent = Some(place);
    let server = TestServer::new(key_door(&key));
    bench.trust(&server);
    let (_session, events) = bench.start(server.config(Auth::Agent));
    connected(&events);
}

#[test]
fn an_agent_with_no_keys_or_none_the_server_takes_says_so() {
    let mut bench = Bench::new("agent-none");
    let place = agent_place(&bench);
    start_agent(place.clone(), Vec::new());
    bench.agent = Some(place);
    let server = TestServer::new(key_door(&new_key(Algorithm::Ed25519)));
    bench.trust(&server);
    let (_session, events) = bench.start(server.config(Auth::Agent));
    assert!(ended(&events).contains("holds no keys"));

    let place = agent_place(&bench);
    start_agent(place.clone(), vec![new_key(Algorithm::Ed25519)]);
    bench.agent = Some(place);
    let (_session, events) = bench.start(server.config(Auth::Agent));
    assert!(ended(&events).contains("did not accept the 1 key"));
}

#[test]
fn an_agent_that_is_not_there_is_said_so() {
    let mut bench = Bench::new("agent-away");
    bench.agent = Some(agent_place(&bench));
    let server = TestServer::new(key_door(&new_key(Algorithm::Ed25519)));
    bench.trust(&server);
    let (_session, events) = bench.start(server.config(Auth::Agent));
    assert!(ended(&events).contains("No SSH agent answers"));
}

#[test]
fn the_shell_ending_ends_the_session_with_its_status() {
    let bench = Bench::new("exit");
    let server = TestServer::new(password_door());
    bench.trust(&server);

    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);
    session.write(b"exit\r".to_vec());
    assert_eq!(ended(&events), "The shell ended.");
    a_few_retries();
    assert_eq!(server.seen().connections, 1);

    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);
    session.write(b"exit 3\r".to_vec());
    assert_eq!(ended(&events), "The shell ended, with status 3.");
}

#[test]
fn new_settings_after_it_ended_connect_again() {
    let bench = Bench::new("after-end");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);
    session.write(b"exit\r".to_vec());
    ended(&events);

    session.reconfigure(server.config(Auth::Password));
    log_in(&session, &events);
}

#[test]
fn asks_and_connects_again_after_the_connection_breaks() {
    let bench = Bench::new("broken");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);

    server.kick.send(()).unwrap();
    assert_eq!(waiting(&events), None);
    // A password is never kept, so it is asked for again.
    let (prompt, _) = asked_secret(&events);
    assert_eq!(prompt, "Password");
    answer_text(&session, PASSWORD);
    connected(&events);
    assert_eq!(server.seen().connections, 2);
}

#[test]
fn waits_for_a_server_that_starts_later() {
    let bench = Bench::new("later");
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    let config = SshConfig {
        host: "127.0.0.1".into(),
        port,
        user: "root".into(),
        ..SshConfig::default()
    };
    let (session, events) = bench.start(config);
    assert_eq!(waiting(&events), None);

    let server = TestServer::on_port(password_door(), port);
    bench.trust(&server);
    log_in(&session, &events);
}

#[test]
fn what_does_not_speak_ssh_is_said_so_and_tried_again() {
    let bench = Bench::new("not-ssh");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = SshConfig {
        host: "127.0.0.1".into(),
        port,
        user: "root".into(),
        ..SshConfig::default()
    };
    let timing = Timing {
        handshake: Duration::from_millis(100),
        ..FAST
    };
    let (_session, events) = bench.start_timed(config, timing);
    // It takes the connection, and says nothing.
    let (quiet, _) = listener.accept().unwrap();
    let problem = waiting(&events).unwrap();
    assert!(problem.contains("No SSH server answers"), "{problem}");
    drop(quiet);
    let _again = listener.accept().unwrap();
}

#[test]
fn another_server_at_the_port_is_said_so_and_tried_again() {
    let bench = Bench::new("http");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = SshConfig {
        host: "127.0.0.1".into(),
        port,
        user: "root".into(),
        ..SshConfig::default()
    };
    let (_session, events) = bench.start(config);
    // It says something else, and closes.
    let (mut other, _) = listener.accept().unwrap();
    std::io::Write::write_all(&mut other, b"HTTP/1.1 400 Bad Request\r\n\r\n").unwrap();
    drop(other);
    let problem = waiting(&events).unwrap();
    assert!(problem.contains(&format!("127.0.0.1:{port}")), "{problem}");
    let _again = listener.accept().unwrap();
}

#[test]
fn a_big_paste_does_not_stop_what_the_shell_prints() {
    let bench = Bench::new("paste");
    let server = TestServer::new(password_door());
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);
    received(&events, "$ ");

    // More than the windows of both ways, so neither side can take it all at once.
    let size = 6 * 1024 * 1024;
    session.write(vec![b'x'; size]);
    let mut count = 0;
    while count < size {
        if let Event::Data(bytes) = expect(&events, "data", |event| matches!(event, Event::Data(_)))
        {
            count += bytes.len();
        }
    }
    assert_eq!(count, size);
}

#[test]
fn missing_settings_say_what_to_fill_in() {
    let bench = Bench::new("missing");
    let (_session, events) = bench.start(SshConfig::default());
    assert!(ended(&events).contains("host"));
}

#[test]
fn no_terminal_ends_the_session_and_says_why() {
    let bench = Bench::new("no-terminal");
    let server = TestServer::new(Door {
        no_terminal: true,
        ..password_door()
    });
    bench.trust(&server);
    let (session, events) = bench.start(server.config(Auth::Password));
    asked_secret(&events);
    answer_text(&session, PASSWORD);
    assert!(ended(&events).contains("did not give a terminal"));
}

#[test]
fn new_settings_connect_to_the_new_place() {
    let bench = Bench::new("move");
    let first = TestServer::new(password_door());
    let second = TestServer::new(password_door());
    bench.trust(&first);
    bench.trust(&second);
    let (session, events) = bench.start(first.config(Auth::Password));
    log_in(&session, &events);

    session.reconfigure(second.config(Auth::Password));
    assert_eq!(waiting(&events), None);
    asked_secret(&events);
    answer_text(&session, PASSWORD);
    assert_eq!(
        connected(&events),
        format!("root@127.0.0.1:{}", second.port)
    );

    // The same place again is still a new connection, and says so.
    session.reconfigure(second.config(Auth::Password));
    assert_eq!(waiting(&events), None);
    log_in(&session, &events);
}

#[test]
fn dropping_the_session_ends_its_thread_also_while_it_asks() {
    let bench = Bench::new("drop");
    let server = TestServer::new(password_door());
    bench.trust(&server);

    let (session, events) = bench.start(server.config(Auth::Password));
    asked_secret(&events);
    let before = Clock::now();
    drop(session);
    assert!(before.elapsed() < Duration::from_secs(1));

    let (session, events) = bench.start(server.config(Auth::Password));
    log_in(&session, &events);
    let before = Clock::now();
    drop(session);
    assert!(before.elapsed() < Duration::from_secs(1));
    while events.try_recv().is_ok() {}
    assert!(matches!(
        events.recv_timeout(Duration::from_millis(50)),
        Err(std_mpsc::RecvTimeoutError::Disconnected)
    ));
}
