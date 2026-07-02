//! `ROOM_DEBUG` per-call capture pipeline.
//!
//! When the service is started with `ROOM_DEBUG=1` it:
//!   * tees its (debug-level) tracing output to
//!     `<dir>/server-<run>.ndjson` — one JSON object per line, every span
//!     field (`call_id`, `session_id`, `source_pid`, …) hoisted to top level;
//!   * accepts per-client debug bundles at `POST /v1/sessions/{id}/debug` and
//!     writes them to `<dir>/<call_id>/client-<participant>-<seq>.json`.
//!
//! The call id shown in the client debug panel is the session UUID, so sharing
//! just that id is enough to reconstruct the full server+client picture of a
//! call: grep the ndjson for the id, read the per-call directory for the
//! clients' diagnostics/log bundles.
//!
//! ## Why a dedicated writer thread
//!
//! Tracing events fire from the tokio runtime AND from the media shard
//! threads (the SFU forwarding hot path). None of those may block on disk.
//! So every event and every uploaded bundle is handed to ONE writer thread
//! over a bounded channel with drop-on-full semantics: under overload we lose
//! debug lines (counted), we never stall a thread that's forwarding media.
//! This mirrors the existing `dispatcher → shard` crossbeam pattern in `sfu`.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam::channel::{Receiver, Sender, TrySendError, bounded};
use parking_lot::Mutex;
use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriter};
use uuid::Uuid;

/// Bounded capture queue. Generous — a 2-person call emits a few hundred
/// events/sec at debug level; 16k entries is ~30s of headroom before we'd
/// start dropping, which only happens if the disk can't keep up at all.
const CHANNEL_CAP: usize = 16_384;

/// Hard cap on a single client bundle. The HTTP route also enforces this via
/// `DefaultBodyLimit`; this is the defence-in-depth copy for any other caller.
pub const MAX_BUNDLE_BYTES: usize = 4 * 1024 * 1024;

/// What the writer thread persists. Server log lines and client bundles share
/// one queue so all capture file I/O is serialized on a single thread.
enum CaptureMsg {
    /// One formatted tracing event (JSON + trailing newline).
    ServerLine(Vec<u8>),
    /// A client's uploaded debug bundle for a call.
    ClientBundle {
        session: Uuid,
        participant: String,
        seq: u64,
        body: Vec<u8>,
    },
}

/// Handle held by `AppState` so the HTTP endpoint can persist client bundles.
/// Cloneable-cheap (everything is behind `Arc`).
pub struct DebugCapture {
    tx: Sender<CaptureMsg>,
    dir: PathBuf,
    /// Per-`(session, participant)` upload counter → stable `…-1.json`,
    /// `…-2.json` ordering so the timeline of a client is reconstructable.
    seq: Mutex<HashMap<(Uuid, String), u64>>,
}

impl DebugCapture {
    /// Create the capture root and spawn the writer thread.
    ///
    /// Returns the handle (for the HTTP endpoint) and a `MakeWriter` to plug
    /// into the tracing layer. `run_id` disambiguates server logs across
    /// restarts so an old file is never silently appended to a new process's
    /// events.
    pub fn init(dir: PathBuf, run_id: &str) -> io::Result<(Arc<Self>, BoxMakeWriter)> {
        std::fs::create_dir_all(&dir)?;
        let server_path = dir.join(format!("server-{run_id}.ndjson"));
        // Fail fast if we can't write where the operator pointed us — better a
        // startup error than a silently empty capture discovered hours later.
        let server_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&server_path)?;

        let (tx, rx) = bounded::<CaptureMsg>(CHANNEL_CAP);
        let writer_dir = dir.clone();
        std::thread::Builder::new()
            .name("room-debug-writer".into())
            .spawn(move || writer_loop(rx, BufWriter::new(server_file), writer_dir))
            .expect("spawn room-debug writer thread");

        let handle = Arc::new(Self {
            tx: tx.clone(),
            dir,
            seq: Mutex::new(HashMap::new()),
        });
        let make_writer = BoxMakeWriter::new(ChannelMakeWriter { tx });
        Ok((handle, make_writer))
    }

    /// Directory capture writes to (for log lines on startup).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Queue a client's debug bundle for persistence. Non-blocking: drops +
    /// counts on a full queue rather than stalling the HTTP handler.
    /// `participant_raw` is sanitized into a filesystem-safe token.
    pub fn record_client_bundle(&self, session: Uuid, participant_raw: &str, body: Vec<u8>) {
        let participant = sanitize_participant(participant_raw);
        let seq = {
            let mut map = self.seq.lock();
            let n = map.entry((session, participant.clone())).or_insert(0);
            *n += 1;
            *n
        };
        let msg = CaptureMsg::ClientBundle {
            session,
            participant,
            seq,
            body,
        };
        if let Err(TrySendError::Full(_)) = self.tx.try_send(msg) {
            // Drop-on-full: the capture queue must never back-pressure the
            // HTTP handler. A non-zero counter means the writer fell behind.
            metrics::counter!("matehub_video_room_debug_drops_total").increment(1);
        }
    }

    /// Evict a destroyed session's upload counters. Without this the
    /// per-(session, participant) seq map only ever grows — one entry per
    /// unique pair for the life of the process.
    pub fn forget_session(&self, session: Uuid) {
        self.seq.lock().retain(|(sid, _), _| *sid != session);
    }
}

/// Drains the capture queue and persists everything. Runs until the channel
/// is disconnected (process shutdown). Batches flushes: one `flush()` per
/// wakeup-burst keeps the ndjson durable (we care most about the log right
/// before a crash) without an fsync per line.
fn writer_loop(rx: Receiver<CaptureMsg>, mut server: BufWriter<std::fs::File>, dir: PathBuf) {
    while let Ok(msg) = rx.recv() {
        handle_msg(&mut server, &dir, msg);
        while let Ok(msg) = rx.try_recv() {
            handle_msg(&mut server, &dir, msg);
        }
        let _ = server.flush();
    }
    let _ = server.flush();
}

fn handle_msg(server: &mut BufWriter<std::fs::File>, dir: &Path, msg: CaptureMsg) {
    match msg {
        CaptureMsg::ServerLine(bytes) => {
            // Best-effort: a failed debug write must not panic the writer
            // thread (which would silently end all capture for the run).
            let _ = server.write_all(&bytes);
        }
        CaptureMsg::ClientBundle {
            session,
            participant,
            seq,
            body,
        } => {
            let call_dir = dir.join(session.to_string());
            if let Err(e) = std::fs::create_dir_all(&call_dir) {
                tracing::warn!(%session, error = %e, "room-debug: cannot create call dir");
                return;
            }
            let path = client_bundle_path(dir, session, &participant, seq);
            if let Err(e) = std::fs::write(&path, &body) {
                tracing::warn!(?path, error = %e, "room-debug: cannot write client bundle");
            }
        }
    }
}

/// Filesystem-safe participant token. The participant id arrives in
/// client-controlled JSON, so we MUST neutralise path traversal (`..`, `/`)
/// before it touches a path. Keep only `[A-Za-z0-9-]`, bound the length.
fn sanitize_participant(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// `<dir>/<session>/client-<participant>-<seq>.json`. Pure so it's testable
/// without spawning the writer thread.
fn client_bundle_path(dir: &Path, session: Uuid, participant: &str, seq: u64) -> PathBuf {
    dir.join(session.to_string())
        .join(format!("client-{participant}-{seq}.json"))
}

/// Pull `participantId` out of a client bundle without fully trusting it —
/// used only to name the file. Returns `None` on non-JSON / missing field;
/// the caller falls back to `"unknown"`.
pub fn participant_id_from_bundle(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("participantId")?.as_str().map(|s| s.to_string())
}

/// `MakeWriter` that funnels each formatted tracing event into the capture
/// queue. The fmt layer calls `make_writer()` once per event, writes the
/// formatted bytes, then drops the writer — so one `LineWriter` == one event.
#[derive(Clone)]
struct ChannelMakeWriter {
    tx: Sender<CaptureMsg>,
}

impl<'a> MakeWriter<'a> for ChannelMakeWriter {
    type Writer = LineWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            tx: self.tx.clone(),
            buf: Vec::with_capacity(256),
        }
    }
}

struct LineWriter {
    tx: Sender<CaptureMsg>,
    buf: Vec<u8>,
}

impl Write for LineWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let buf = std::mem::take(&mut self.buf);
        // Drop-on-full: never block the thread that emitted the event. Server
        // lines dominate the queue (per-packet `video rtp in/out`), so a
        // silent drop shows up as a gap in server-*.ndjson that reads exactly
        // like "the SFU never forwarded these packets" — count it, so a
        // capture with drops_total > 0 is known-incomplete. Full only:
        // Disconnected during shutdown is not a lost line worth alarming on.
        if let Err(TrySendError::Full(_)) = self.tx.try_send(CaptureMsg::ServerLine(buf)) {
            metrics::counter!("matehub_video_room_debug_drops_total").increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_participant_strips_path_traversal() {
        assert_eq!(sanitize_participant("../../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_participant("a/b\\c"), "abc");
        assert_eq!(sanitize_participant(""), "unknown");
        assert_eq!(sanitize_participant("///"), "unknown");
    }

    #[test]
    fn sanitize_participant_keeps_uuid_shape() {
        let id = "ec0dc302-809d-532e-ba11-6a717d3e8739";
        assert_eq!(sanitize_participant(id), id);
    }

    #[test]
    fn sanitize_participant_bounds_length() {
        let long = "a".repeat(200);
        assert_eq!(sanitize_participant(&long).len(), 64);
    }

    #[test]
    fn client_bundle_path_is_under_call_dir() {
        let session = Uuid::nil();
        let p = client_bundle_path(Path::new("/tmp/rd"), session, "pid1", 3);
        assert_eq!(
            p,
            Path::new("/tmp/rd")
                .join("00000000-0000-0000-0000-000000000000")
                .join("client-pid1-3.json")
        );
    }

    #[test]
    fn participant_id_extracted_from_bundle() {
        let body = br#"{"participantId":"abc-123","sessionId":"x","logs":[]}"#;
        assert_eq!(
            participant_id_from_bundle(body),
            Some("abc-123".to_string())
        );
    }

    #[test]
    fn participant_id_none_on_garbage_or_missing() {
        assert_eq!(participant_id_from_bundle(b"not json"), None);
        assert_eq!(participant_id_from_bundle(br#"{"sessionId":"x"}"#), None);
        assert_eq!(
            participant_id_from_bundle(br#"{"participantId":123}"#),
            None
        );
    }

    #[test]
    fn record_client_bundle_writes_file_and_increments_seq() {
        let tmp = std::env::temp_dir().join(format!("rd-test-{}", Uuid::new_v4()));
        let (capture, _mw) = DebugCapture::init(tmp.clone(), "test").expect("init");
        let session = Uuid::new_v4();
        capture.record_client_bundle(session, "pid-a", b"{\"participantId\":\"pid-a\"}".to_vec());
        capture.record_client_bundle(session, "pid-a", b"{\"participantId\":\"pid-a\"}".to_vec());

        // Writer thread is async; poll briefly for the second file to appear.
        let target = tmp.join(session.to_string()).join("client-pid-a-2.json");
        let mut ok = false;
        for _ in 0..200 {
            if target.exists() {
                ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ok, "expected {target:?} to be written");
        // First file must also exist (seq started at 1).
        assert!(
            tmp.join(session.to_string())
                .join("client-pid-a-1.json")
                .exists()
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
