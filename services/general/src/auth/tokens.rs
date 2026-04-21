use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Generate a one-shot verification token. 122 bits of entropy (UUID v4),
/// comfortably more than the bcrypt work factor we protect passwords with
/// and plenty for a 24h single-use link.
pub fn generate_verification_token() -> String {
    Uuid::new_v4().to_string()
}

/// SHA-256 hash the token before storing. This way a DB leak doesn't hand
/// an attacker working verification links — they'd need to preimage the
/// hash, and SHA-256 isn't going to oblige.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}
