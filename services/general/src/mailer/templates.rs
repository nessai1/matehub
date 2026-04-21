use anyhow::{Context, Result};
use askama::Template;
use serde::Deserialize;

use super::Mail;

// ── Template structs ──────────────────────────────────────────────────
// One pair (html + txt) per logical template name. The template path is
// relative to `templates/` at the crate root (askama default).

#[derive(Template)]
#[template(path = "account_verify.html")]
struct AccountVerifyHtml<'a> {
    verify_url: &'a str,
}

#[derive(Template)]
#[template(path = "account_verify.txt")]
struct AccountVerifyText<'a> {
    verify_url: &'a str,
}

#[derive(Template)]
#[template(path = "account_welcome.html")]
struct AccountWelcomeHtml<'a> {
    dashboard_url: &'a str,
}

#[derive(Template)]
#[template(path = "account_welcome.txt")]
struct AccountWelcomeText<'a> {
    dashboard_url: &'a str,
}

#[derive(Template)]
#[template(path = "hub_invite.html")]
struct HubInviteHtml<'a> {
    hub_name: &'a str,
    inviter_name: &'a str,
    invite_url: &'a str,
}

#[derive(Template)]
#[template(path = "hub_invite.txt")]
struct HubInviteText<'a> {
    hub_name: &'a str,
    inviter_name: &'a str,
    invite_url: &'a str,
}

// ── Payload shapes ────────────────────────────────────────────────────
// These mirror what the caller enqueues in `outbox::enqueue_tx`. Keeping
// them typed means a payload schema drift is a compile error, not a
// 3 AM pager.

#[derive(Deserialize)]
struct AccountVerifyPayload {
    verify_url: String,
}

#[derive(Deserialize)]
struct AccountWelcomePayload {
    dashboard_url: String,
}

#[derive(Deserialize)]
struct HubInvitePayload {
    hub_name: String,
    inviter_name: String,
    invite_url: String,
}

// ── Dispatch ──────────────────────────────────────────────────────────

pub fn render(
    template: &str,
    to_email: &str,
    payload: &serde_json::Value,
    mail_from: &str,
) -> Result<Mail> {
    match template {
        "account_verify" => {
            let p: AccountVerifyPayload = serde_json::from_value(payload.clone())
                .context("invalid payload for account_verify")?;
            Ok(Mail {
                to: to_email.to_string(),
                from: mail_from.to_string(),
                subject: "Confirm your MateHub email".into(),
                html: AccountVerifyHtml {
                    verify_url: &p.verify_url,
                }
                .render()?,
                text: AccountVerifyText {
                    verify_url: &p.verify_url,
                }
                .render()?,
            })
        }
        "account_welcome" => {
            let p: AccountWelcomePayload = serde_json::from_value(payload.clone())
                .context("invalid payload for account_welcome")?;
            Ok(Mail {
                to: to_email.to_string(),
                from: mail_from.to_string(),
                subject: "Welcome to MateHub".into(),
                html: AccountWelcomeHtml {
                    dashboard_url: &p.dashboard_url,
                }
                .render()?,
                text: AccountWelcomeText {
                    dashboard_url: &p.dashboard_url,
                }
                .render()?,
            })
        }
        "hub_invite" => {
            let p: HubInvitePayload = serde_json::from_value(payload.clone())
                .context("invalid payload for hub_invite")?;
            Ok(Mail {
                to: to_email.to_string(),
                from: mail_from.to_string(),
                subject: format!("You're invited to {} on MateHub", p.hub_name),
                html: HubInviteHtml {
                    hub_name: &p.hub_name,
                    inviter_name: &p.inviter_name,
                    invite_url: &p.invite_url,
                }
                .render()?,
                text: HubInviteText {
                    hub_name: &p.hub_name,
                    inviter_name: &p.inviter_name,
                    invite_url: &p.invite_url,
                }
                .render()?,
            })
        }
        other => anyhow::bail!("unknown mail template: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_verify() {
        let mail = render(
            "account_verify",
            "user@example.com",
            &serde_json::json!({ "verify_url": "https://matehub.io/verify-email?token=abc" }),
            "MateHub <noreply@matehub.io>",
        )
        .unwrap();
        assert!(mail.html.contains("Confirm email"));
        assert!(mail.html.contains("https://matehub.io/verify-email?token=abc"));
        assert!(mail.text.contains("https://matehub.io/verify-email?token=abc"));
    }

    #[test]
    fn rejects_unknown_template() {
        let err = render(
            "nonexistent",
            "u@e.com",
            &serde_json::json!({}),
            "x@y.z",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown mail template"));
    }

    #[test]
    fn payload_mismatch_errors() {
        // account_verify needs verify_url, not dashboard_url
        let err = render(
            "account_verify",
            "u@e.com",
            &serde_json::json!({ "dashboard_url": "x" }),
            "x@y.z",
        )
        .unwrap_err();
        assert!(err.to_string().contains("account_verify"));
    }
}
