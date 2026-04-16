use std::sync::Mutex;

use sonyflake::Sonyflake;

/// Thread-safe Snowflake ID generator.
/// IDs are 64-bit, chronologically sortable, unique across processes.
static GENERATOR: Mutex<Option<Sonyflake>> = Mutex::new(None);

/// 7-day bucket in seconds
const BUCKET_INTERVAL_SECS: u64 = 7 * 24 * 3600;

pub fn init() {
    let sf = Sonyflake::new().expect("failed to create Snowflake generator");
    *GENERATOR.lock().unwrap() = Some(sf);
    tracing::info!("Snowflake ID generator initialized");
}

/// Generate a new Snowflake ID. Panics if not initialized.
pub fn next_id() -> i64 {
    GENERATOR
        .lock()
        .unwrap()
        .as_mut()
        .expect("Snowflake not initialized -- call snowflake::init() first")
        .next_id()
        .expect("Snowflake ID generation failed") as i64
}

/// Current time bucket (for ScyllaDB partition key).
/// All messages written "now" go into this bucket.
/// 7-day windows, counting from Unix epoch.
pub fn current_bucket() -> i32 {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    (now_secs / BUCKET_INTERVAL_SECS) as i32
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
        let b = current_bucket();
        assert!(b > 0, "bucket must be positive: {b}");
    }

    #[test]
    fn bucket_is_stable_within_call() {
        let b1 = current_bucket();
        let b2 = current_bucket();
        assert_eq!(b1, b2);
    }
}
