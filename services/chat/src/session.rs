//! Session buffer for RESUME support.
//!
//! Each gateway session keeps a ring-buffer of recently dispatched events
//! (serialized JSON bytes). When a client reconnects with RESUME, the
//! server replays events from `last_seq + 1` without re-fetching from DB.
//!
//! Buffer cap: 512 events or ~2 MB (whichever hit first).
//! TTL: 5 minutes after disconnect. Cleaned by a background reaper.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::auth::Claims;

const MAX_EVENTS: usize = 512;
const MAX_BYTES: usize = 2 * 1024 * 1024; // 2 MB
const SESSION_TTL: Duration = Duration::from_secs(300); // 5 min

/// A single buffered event: sequence number + serialized JSON.
#[derive(Clone)]
struct BufferedEvent {
    seq: u64,
    data: Vec<u8>, // JSON bytes of GatewayEvent
}

/// Per-session replay buffer.
struct SessionInner {
    /// JWT claims from IDENTIFY (user info, hub_id)
    claims: Claims,
    /// NATS subject this session was subscribed to
    nats_subject: String,
    /// Ring-buffer of dispatched events
    events: VecDeque<BufferedEvent>,
    /// Total byte size of buffered events
    total_bytes: usize,
    /// Last sequence number dispatched
    last_seq: u64,
    /// When the session was disconnected (None = still connected)
    disconnected_at: Option<Instant>,
}

impl SessionInner {
    fn push(&mut self, seq: u64, data: Vec<u8>) {
        let len = data.len();
        self.events.push_back(BufferedEvent { seq, data });
        self.total_bytes += len;
        self.last_seq = seq;

        // Evict oldest if over capacity
        while self.events.len() > MAX_EVENTS || self.total_bytes > MAX_BYTES {
            if let Some(old) = self.events.pop_front() {
                self.total_bytes -= old.data.len();
            }
        }
    }

    /// Replay events from `after_seq + 1`. Returns None if seq is outside buffer.
    fn replay_from(&self, after_seq: u64) -> Option<Vec<Vec<u8>>> {
        if after_seq > self.last_seq {
            return Some(vec![]); // client is ahead (shouldn't happen, but harmless)
        }

        // Find start position
        let min_seq = self
            .events
            .front()
            .map(|e| e.seq)
            .unwrap_or(self.last_seq + 1);

        if after_seq + 1 < min_seq {
            // Gap: requested seq is before the buffer window
            return None;
        }

        let replay: Vec<Vec<u8>> = self
            .events
            .iter()
            .filter(|e| e.seq > after_seq)
            .map(|e| e.data.clone())
            .collect();

        Some(replay)
    }
}

/// Handle to a session's buffer. Cheap to clone (Arc<Mutex<...>>).
#[derive(Clone)]
pub struct SessionHandle(Arc<Mutex<SessionInner>>);

impl SessionHandle {
    /// Buffer a dispatched event.
    pub fn push(&self, seq: u64, data: Vec<u8>) {
        self.0.lock().push(seq, data);
    }

    /// Mark as disconnected (starts the TTL countdown).
    pub fn mark_disconnected(&self) {
        self.0.lock().disconnected_at = Some(Instant::now());
    }

    /// Mark as re-connected.
    pub fn mark_connected(&self) {
        self.0.lock().disconnected_at = None;
    }

    /// Get claims for this session.
    pub fn claims(&self) -> Claims {
        self.0.lock().claims.clone()
    }

    /// Get NATS subject for this session.
    pub fn nats_subject(&self) -> String {
        self.0.lock().nats_subject.clone()
    }

    /// Get last dispatched sequence number.
    pub fn last_seq(&self) -> u64 {
        self.0.lock().last_seq
    }

    /// Try to replay events after `after_seq`. Returns None if gap.
    pub fn replay_from(&self, after_seq: u64) -> Option<Vec<Vec<u8>>> {
        self.0.lock().replay_from(after_seq)
    }
}

/// Shared session store. Lives in AppState.
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<Mutex<std::collections::HashMap<String, SessionHandle>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Create a new session and return its ID + handle.
    pub fn create(&self, claims: Claims, nats_subject: String) -> (String, SessionHandle) {
        let session_id = uuid::Uuid::new_v4().to_string();
        let handle = SessionHandle(Arc::new(Mutex::new(SessionInner {
            claims,
            nats_subject,
            events: VecDeque::new(),
            total_bytes: 0,
            last_seq: 0,
            disconnected_at: None,
        })));
        self.sessions
            .lock()
            .insert(session_id.clone(), handle.clone());
        (session_id, handle)
    }

    /// Look up a session by ID for RESUME.
    pub fn get(&self, session_id: &str) -> Option<SessionHandle> {
        self.sessions.lock().get(session_id).cloned()
    }

    /// Remove a session (e.g., after TTL expiry).
    pub fn remove(&self, session_id: &str) {
        self.sessions.lock().remove(session_id);
    }

    /// Reap expired sessions (disconnected > TTL). Call from background task.
    pub fn reap_expired(&self) -> usize {
        let mut sessions = self.sessions.lock();
        let before = sessions.len();
        sessions.retain(|_, handle| {
            let inner = handle.0.lock();
            match inner.disconnected_at {
                Some(t) => t.elapsed() < SESSION_TTL,
                None => true, // still connected
            }
        });
        before - sessions.len()
    }

    /// Number of active sessions (for metrics).
    pub fn len(&self) -> usize {
        self.sessions.lock().len()
    }
}
