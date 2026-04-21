//! Snowflake ID generator shared across all matehub services.
//!
//! Layout (Sonyflake, 63-bit positive i64):
//!   time(39 bits, 10ms units since epoch 2014-09-01) | sequence(8) | machine_id(16)
//!
//! Machine ID resolution priority:
//!   1. `MACHINE_ID` env var (explicit override, 0..=65535)
//!   2. Lower 16 bits of the first private-range IPv4 address (Sonyflake default)
//!
//! In K8s each pod gets a unique private IP, so (2) gives collision-free IDs
//! across pods without coordination. `MACHINE_ID` exists for deterministic
//! local testing and for on-prem deployments where the default picker fails.
//!
//! 7-day bucket helpers (`current_bucket`, `bucket_from_id`) live here because
//! they're intrinsic to the ID's time domain — the bucket *is* a coarse
//! quantization of the timestamp embedded in the Snowflake.

use std::sync::Mutex;

use sonyflake::Sonyflake;

static GENERATOR: Mutex<Option<Sonyflake>> = Mutex::new(None);

/// Sonyflake epoch: 2014-09-01T00:00:00Z in Unix ms.
pub const SONYFLAKE_EPOCH_MS: i64 = 1_409_529_600_000;

/// 7-day bucket window, in seconds. Used by chat's ScyllaDB partition key.
pub const BUCKET_INTERVAL_SECS: u64 = 7 * 24 * 3600;

pub fn init() {
    let env_id = std::env::var("MACHINE_ID")
        .ok()
        .and_then(|s| s.parse::<u16>().ok());

    let sf = if let Some(id) = env_id {
        Sonyflake::builder()
            .machine_id(&|| Ok(id))
            .finalize()
            .expect("failed to create Snowflake generator with MACHINE_ID override")
    } else {
        Sonyflake::new().expect("failed to create Snowflake generator (no private IP?)")
    };

    *GENERATOR.lock().unwrap() = Some(sf);
    tracing::info!(machine_id = ?env_id, "Snowflake generator initialized");
}

/// Generate a new Snowflake ID. Panics if `init()` was not called.
pub fn next_id() -> i64 {
    GENERATOR
        .lock()
        .unwrap()
        .as_mut()
        .expect("Snowflake not initialized — call snowflake::init() first")
        .next_id()
        .expect("Snowflake ID generation failed") as i64
}

/// Current 7-day bucket (for ScyllaDB partition key). All IDs generated "now"
/// hash into this bucket.
pub fn current_bucket() -> i32 {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    (now_secs / BUCKET_INTERVAL_SECS) as i32
}

/// Extract the bucket that a given Snowflake ID falls into.
///
/// Sonyflake layout: the upper 39 bits are "10ms since SONYFLAKE_EPOCH_MS",
/// which we shift out by dividing by 2^24. Converting to Unix seconds and
/// dividing by BUCKET_INTERVAL_SECS gives the same bucket that would have
/// been written to ScyllaDB at insert time.
pub fn bucket_from_id(id: i64) -> i32 {
    let time_10ms = id >> 24;
    let unix_ms = SONYFLAKE_EPOCH_MS + time_10ms * 10;
    let unix_secs = (unix_ms / 1000) as u64;
    (unix_secs / BUCKET_INTERVAL_SECS) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic() {
        init();
        let a = next_id();
        let b = next_id();
        assert!(b > a, "IDs must be monotonically increasing");
    }

    #[test]
    fn current_bucket_is_positive() {
        assert!(current_bucket() > 0);
    }

    #[test]
    fn bucket_from_fresh_id_equals_current_bucket() {
        init();
        let id = next_id();
        assert_eq!(bucket_from_id(id), current_bucket());
    }
}
