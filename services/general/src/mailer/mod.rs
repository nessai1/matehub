pub mod outbox;
pub mod templates;
pub mod transport;
pub mod worker;

/// A rendered mail message ready for the transport layer to ship.
/// Templates produce this; transport doesn't care how it got built.
#[derive(Debug)]
pub struct Mail {
    pub to: String,
    pub from: String,
    pub subject: String,
    pub html: String,
    pub text: String,
}
