//! Tracks connected receivers and the UDP address each one is reachable at.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::ids;

pub type SessionId = u64;

struct Session {
    name: String,
    media: Option<SocketAddr>,
    punch: Option<Sender<SocketAddr>>,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<SessionId, Session>,
    static_targets: Vec<SocketAddr>,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves a session and returns the channel that the punch listener signals.
    pub fn open(&self, name: String) -> (SessionId, Receiver<SocketAddr>) {
        let (tx, rx) = channel();
        let id = ids::random_u64();
        let session = Session {
            name,
            media: None,
            punch: Some(tx),
        };
        self.inner.lock().unwrap().sessions.insert(id, session);
        (id, rx)
    }

    pub fn attach_media(&self, id: SessionId, addr: SocketAddr) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.sessions.get_mut(&id) else {
            return false;
        };
        session.media = Some(addr);
        if let Some(punch) = session.punch.take() {
            let _ = punch.send(addr);
        }
        true
    }

    pub fn close(&self, id: SessionId) -> Option<String> {
        let session = self.inner.lock().unwrap().sessions.remove(&id)?;
        Some(session.name)
    }

    pub fn add_static_target(&self, addr: SocketAddr) {
        self.inner.lock().unwrap().static_targets.push(addr);
    }

    /// Holds the registry lock for the duration of the fan-out, which runs once
    /// per 20 ms frame and only ever calls a non-blocking `send_to`.
    pub fn for_each_target(&self, mut f: impl FnMut(SocketAddr)) {
        let inner = self.inner.lock().unwrap();
        for addr in &inner.static_targets {
            f(*addr);
        }
        for session in inner.sessions.values() {
            if let Some(addr) = session.media {
                f(addr);
            }
        }
    }
}
