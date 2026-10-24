//! The serial commands the UI calls, and the sessions they run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};
use ternimal_core::serial::{
    Event, ModemLines, PortId, PortInfo, SerialConfig, Session, SessionState, SystemPorts,
};

/// The running sessions, by the number the UI gave each.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<u32, Session>>);

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, HashMap<u32, Session>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `action` on a session, if it is running.
    fn with(&self, id: u32, action: impl FnOnce(&Session)) {
        if let Some(session) = self.lock().get(&id) {
            action(session);
        }
    }
}

/// What a session tells the UI, apart from the data it receives.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    State(SessionState),
    Lines(ModemLines),
}

#[tauri::command(async)]
pub fn list_ports() -> Vec<PortInfo> {
    ternimal_core::serial::list_ports()
}

/// Starts a session. Received bytes go to `data` as raw bytes, everything else to `events`.
#[tauri::command(async)]
pub fn open_session(
    id: u32,
    port: PortId,
    config: SerialConfig,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    sessions: State<Sessions>,
) {
    let session = Session::start(Arc::new(SystemPorts), port, config, move |event| {
        // A send only fails when the window is gone, and then nobody is listening.
        let _ = match event {
            Event::Data(bytes) => data.send(InvokeResponseBody::Raw(bytes)),
            Event::State(state) => events.send(SessionEvent::State(state)),
            Event::Lines(lines) => events.send(SessionEvent::Lines(lines)),
        };
    });
    // A session with the same number is replaced. Closing it can take a moment,
    // so it happens after the lock is let go.
    let old = sessions.lock().insert(id, session);
    drop(old);
}

#[tauri::command(async)]
pub fn close_session(id: u32, sessions: State<Sessions>) {
    let old = sessions.lock().remove(&id);
    drop(old);
}

/// Closes every session. The UI calls it when it loads, so a reload never
/// leaves a session holding its port.
#[tauri::command(async)]
pub fn close_all_sessions(sessions: State<Sessions>) {
    let old = std::mem::take(&mut *sessions.lock());
    drop(old);
}

/// Sends bytes to a device. Not async, so key presses go out in the order they were typed.
#[tauri::command]
pub fn write_session(id: u32, data: Vec<u8>, sessions: State<Sessions>) {
    sessions.with(id, |session| session.write(data));
}

#[tauri::command]
pub fn set_session_lines(id: u32, dtr: Option<bool>, rts: Option<bool>, sessions: State<Sessions>) {
    sessions.with(id, |session| {
        if let Some(on) = dtr {
            session.set_dtr(on);
        }
        if let Some(on) = rts {
            session.set_rts(on);
        }
    });
}

#[tauri::command]
pub fn break_session(id: u32, sessions: State<Sessions>) {
    sessions.with(id, Session::send_break);
}

#[tauri::command]
pub fn reconfigure_session(id: u32, config: SerialConfig, sessions: State<Sessions>) {
    sessions.with(id, |session| session.reconfigure(config));
}
