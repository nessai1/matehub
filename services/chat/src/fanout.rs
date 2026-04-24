use crate::models::Message;

/// Fan-out service: publishes chat events to NATS for real-time delivery.
/// Gateway nodes subscribe to relevant subjects and push to WebSocket.
pub struct FanoutService {
    pub(crate) nats: async_nats::Client,
}

impl FanoutService {
    pub fn new(nats: async_nats::Client) -> Self {
        Self { nats }
    }

    /// Publish MESSAGE_CREATE to NATS subject `hub.{hub_id}.channel.{channel_id}.message`
    pub async fn publish_message(&self, msg: &Message) {
        let subject = format!(
            "hub.{}.channel.{}.message",
            msg.hub_id, msg.channel_id
        );

        let payload = match serde_json::to_vec(msg) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Failed to serialize message for NATS: {e}");
                return;
            }
        };

        if let Err(e) = self.nats.publish(subject.clone(), payload.into()).await {
            tracing::error!(%subject, "NATS publish failed: {e}");
        }
    }

    /// Publish generic event (edit, delete, etc.)
    pub async fn publish_event(&self, hub_id: i64, channel_id: i64, event_type: &str, payload: &serde_json::Value) {
        let subject = format!("hub.{hub_id}.channel.{channel_id}.{event_type}");
        if let Ok(data) = serde_json::to_vec(payload) {
            let _ = self.nats.publish(subject, data.into()).await;
        }
    }

    /// Publish typing indicator (ephemeral, no persistence)
    pub async fn publish_typing(&self, hub_id: i64, channel_id: i64, user_id: &str) {
        let subject = format!("hub.{hub_id}.channel.{channel_id}.typing");
        let payload = serde_json::json!({
            "user_id": user_id,
            // Stringified so the JS consumer doesn't silently round
            // Snowflake IDs on parse.
            "channel_id": channel_id.to_string(),
        });

        let _ = self
            .nats
            .publish(subject, serde_json::to_vec(&payload).unwrap().into())
            .await;
    }
}
