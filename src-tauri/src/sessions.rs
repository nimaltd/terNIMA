//! The session commands the UI calls, for serial ports and for RTT, and the
//! sessions they run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use ternimal_core::{probe, rtt, serial, swo};

/// A running session of either kind.
enum Running {
    Serial(serial::Session),
    Rtt(rtt::Session),
    Swo(swo::Session),
}

/// The running sessions, by the number the UI gave each.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<u32, Running>>);

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, HashMap<u32, Running>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `action` on a session, if it is running.
    fn with(&self, id: u32, action: impl FnOnce(&Running)) {
        if let Some(session) = self.lock().get(&id) {
            action(session);
        }
    }

    /// Adds a session. One with the same number is replaced; closing it can
    /// take a moment, so it happens after the lock is let go.
    fn insert(&self, id: u32, session: Running) {
        let old = self.lock().insert(id, session);
        drop(old);
    }
}

/// What a session tells the UI, apart from the data it receives.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    State(serial::SessionState),
    Lines(serial::ModemLines),
    RttState(rtt::RttState),
    Channels {
        up: Vec<rtt::ChannelInfo>,
        down: Vec<rtt::ChannelInfo>,
    },
    SwoState(swo::SwoState),
    /// The chip's trace FIFO overflowed this many times since the last report.
    Overflow {
        count: u32,
    },
}

#[tauri::command(async)]
pub fn list_ports() -> Vec<serial::PortInfo> {
    serial::list_ports()
}

#[tauri::command(async)]
pub fn list_probes() -> Vec<probe::ProbeInfo> {
    probe::list_probes()
}

#[tauri::command(async)]
pub fn search_chips(text: String, limit: usize) -> Vec<String> {
    probe::search_chips(&text, limit)
}

/// Starts a serial session. Received bytes go to `data` as raw bytes, everything else to `events`.
#[tauri::command(async)]
pub fn open_session(
    id: u32,
    port: serial::PortId,
    config: serial::SerialConfig,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    sessions: State<Sessions>,
) {
    let session =
        serial::Session::start(Arc::new(serial::SystemPorts), port, config, move |event| {
            // A send only fails when the window is gone, and then nobody is listening.
            let _ = match event {
                serial::Event::Data(bytes) => data.send(InvokeResponseBody::Raw(bytes)),
                serial::Event::State(state) => events.send(SessionEvent::State(state)),
                serial::Event::Lines(lines) => events.send(SessionEvent::Lines(lines)),
            };
        });
    sessions.insert(id, Running::Serial(session));
}

/// Starts an RTT session, with the same channels as a serial one.
#[tauri::command(async)]
pub fn open_rtt_session(
    id: u32,
    config: rtt::RttConfig,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    sessions: State<Sessions>,
) {
    let session = rtt::Session::start(Arc::new(rtt::ProbeRs), config, move |event| {
        let _ = match event {
            rtt::Event::Data(bytes) => data.send(InvokeResponseBody::Raw(bytes)),
            rtt::Event::State(state) => events.send(SessionEvent::RttState(state)),
            rtt::Event::Channels { up, down } => events.send(SessionEvent::Channels { up, down }),
        };
    });
    sessions.insert(id, Running::Rtt(session));
}

/// Starts an SWO session, with the same channels as the others. It only receives.
#[tauri::command(async)]
pub fn open_swo_session(
    id: u32,
    config: swo::SwoConfig,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    sessions: State<Sessions>,
) {
    let session = swo::Session::start(Arc::new(swo::ProbeRs), config, move |event| {
        let _ = match event {
            swo::Event::Data(bytes) => data.send(InvokeResponseBody::Raw(bytes)),
            swo::Event::State(state) => events.send(SessionEvent::SwoState(state)),
            swo::Event::Overflow(count) => events.send(SessionEvent::Overflow { count }),
        };
    });
    sessions.insert(id, Running::Swo(session));
}

#[tauri::command(async)]
pub fn close_session(id: u32, sessions: State<Sessions>) {
    let old = sessions.lock().remove(&id);
    drop(old);
}

/// Closes every session. The UI calls it when it loads, so a reload never
/// leaves a session holding its port or probe.
#[tauri::command(async)]
pub fn close_all_sessions(sessions: State<Sessions>) {
    let old = std::mem::take(&mut *sessions.lock());
    drop(old);
}

/// Sends bytes to a device. Not async, so key presses go out in the order they were typed.
#[tauri::command]
pub fn write_session(id: u32, data: Vec<u8>, sessions: State<Sessions>) {
    sessions.with(id, |session| match session {
        Running::Serial(session) => session.write(data),
        Running::Rtt(session) => session.write(data),
        // SWO only comes from the chip.
        Running::Swo(_) => {}
    });
}

#[tauri::command]
pub fn set_session_lines(id: u32, dtr: Option<bool>, rts: Option<bool>, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Serial(session) = session {
            if let Some(on) = dtr {
                session.set_dtr(on);
            }
            if let Some(on) = rts {
                session.set_rts(on);
            }
        }
    });
}

#[tauri::command]
pub fn break_session(id: u32, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Serial(session) = session {
            session.send_break();
        }
    });
}

#[tauri::command]
pub fn reconfigure_session(id: u32, config: serial::SerialConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Serial(session) = session {
            session.reconfigure(config);
        }
    });
}

#[tauri::command]
pub fn reconfigure_rtt_session(id: u32, config: rtt::RttConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Rtt(session) = session {
            session.reconfigure(config);
        }
    });
}

#[tauri::command]
pub fn reconfigure_swo_session(id: u32, config: swo::SwoConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Running::Swo(session) = session {
            session.reconfigure(config);
        }
    });
}

/// Resets the chip of an RTT or SWO session.
#[tauri::command]
pub fn reset_target(id: u32, sessions: State<Sessions>) {
    sessions.with(id, |session| match session {
        Running::Rtt(session) => session.reset(),
        Running::Swo(session) => session.reset(),
        Running::Serial(_) => {}
    });
}
