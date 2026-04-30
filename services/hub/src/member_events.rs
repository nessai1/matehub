//! Hub-internal broadcast for member-list change events.
//!
//! Reuses the same `presence::PresenceEvent` channel that powers voice
//! occupancy, so connected presence-WS clients get every change in one
//! socket. Used by the frontend's SWR cache to invalidate the members
//! list without polling.
//!
//! Pattern mirrors `acl_publish.rs`: a process-global `OnceLock<Sender>`
//! that any write-path can call without threading a sender through every
//! handler's State{}. Pre-init or no-receivers = silent no-op.

use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::api::presence_ws::PresenceEvent;

static SENDER: OnceLock<broadcast::Sender<PresenceEvent>> = OnceLock::new();

pub fn init(sender: broadcast::Sender<PresenceEvent>) {
    let _ = SENDER.set(sender);
}

/// Wire format. `kind` is the discriminator the frontend switches on.
/// IDs go out as decimal strings -- Snowflakes overflow JS safe-int.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    MemberJoined {
        #[serde(with = "matehub_common::serde_i64::as_string")]
        user_id: i64,
    },
    MemberLeft {
        #[serde(with = "matehub_common::serde_i64::as_string")]
        user_id: i64,
    },
    MemberGroupsChanged {
        #[serde(with = "matehub_common::serde_i64::as_string")]
        user_id: i64,
    },
}

pub fn member_joined(hub_id: i64, user_id: i64) {
    publish(hub_id, WireEvent::MemberJoined { user_id });
}

pub fn member_left(hub_id: i64, user_id: i64) {
    publish(hub_id, WireEvent::MemberLeft { user_id });
}

pub fn member_groups_changed(hub_id: i64, user_id: i64) {
    publish(hub_id, WireEvent::MemberGroupsChanged { user_id });
}

fn publish(hub_id: i64, ev: WireEvent) {
    let Some(sender) = SENDER.get() else { return };
    let payload = match serde_json::to_string(&ev) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "member_events: serialize failed");
            return;
        }
    };
    // SendError just means there are no live subscribers (no presence WS
    // for this hub right now). That's fine -- the next reconnect will
    // re-fetch via SWR.
    let _ = sender.send(PresenceEvent { hub_id, payload });
}
