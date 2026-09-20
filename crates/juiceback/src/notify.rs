//! notification system for reports (discord webhook + SMTP email).
use std::{fmt::Write as _, sync::Arc};

use crate::{config::Config, state::AppState};

pub fn dispatch_report_notifications(
    state: &Arc<AppState>,
    config: &Arc<Config>,
    file_url: &str,
    reason: &str,
    details: &str,
    reporter_ip: Option<&str>,
) {
    if let Some(ref webhook_url) = config.report_webhook_url {
        let semaphore = state.notification_semaphore.clone();
        let url = webhook_url.clone();
        let file_url = file_url.to_string();
        let reason = reason.to_string();
        let details = details.to_string();
        let ip = reporter_ip.map(ToString::to_string);
        tokio::spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            if let Err(e) = send_webhook(&url, &file_url, &reason, &details, ip.as_deref()).await {
                tracing::warn!("webhook notification failed: {e}");
            }
        });
    }

    if let Some(ref recipient) = config.report_email_recipient
        && let (Some(host), Some(sender)) = (&config.smtp_host, &config.report_email_sender)
    {
        let semaphore = state.notification_semaphore.clone();
        let host = host.clone();
        let port = config.smtp_port.unwrap_or(465);
        let username = config.smtp_username.clone();
        let password = config.smtp_password.clone();
        let recipient = recipient.clone();
        let sender = sender.clone();
        let file_url = file_url.to_string();
        let reason = reason.to_string();
        let details = details.to_string();
        let ip = reporter_ip.map(ToString::to_string);
        tokio::spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            let mut body = format!("Report submitted for: {file_url}\nReason: {reason}");
            if !details.is_empty() {
                let _ = write!(body, "\nDetails: {details}");
            }
            if let Some(ref ip) = ip {
                let _ = write!(body, "\nReporter (hashed): {ip}");
            }
            if let Err(e) = send_smtp_email(
                &host,
                port,
                username.as_deref(),
                password.as_deref(),
                &sender,
                &recipient,
                &format!("Juicebox Report: {reason}"),
                &body,
            )
            .await
            {
                tracing::warn!("email notification failed: {e}");
            }
        });
    }
}

/// Send a confirmation email to the reporter letting them know their report is
/// under review.
pub fn send_reporter_confirmation(
    state: &Arc<AppState>,
    config: &Arc<Config>,
    to_email: &str,
    file_url: &str,
    reason: &str,
) {
    if let (Some(host), Some(sender)) = (&config.smtp_host, &config.report_email_sender) {
        let semaphore = state.notification_semaphore.clone();
        let host = host.clone();
        let port = config.smtp_port.unwrap_or(465);
        let username = config.smtp_username.clone();
        let password = config.smtp_password.clone();
        let sender = sender.clone();
        let to_email = to_email.to_string();
        let file_url = file_url.to_string();
        let reason = reason.to_string();
        tokio::spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            let body = format!(
                "Hi,\n\n\
                 We received your report for:\n\
                 URL: {file_url}\n\
                 Reason: {reason}\n\n\
                 Your report is now under review. We appreciate you helping keep the platform safe.\n\n\
                 - Juicebox Team",
            );
            if let Err(e) = send_smtp_email(
                &host,
                port,
                username.as_deref(),
                password.as_deref(),
                &sender,
                &to_email,
                "Your report is under review",
                &body,
            )
            .await
            {
                tracing::warn!("reporter confirmation email failed: {e}");
            }
        });
    }
}

async fn send_webhook(
    url: &str,
    file_url: &str,
    reason: &str,
    details: &str,
    reporter_ip: Option<&str>,
) -> Result<(), String> {
    let mut description = format!("**Reason:** {reason}\n**URL:** {file_url}");
    if !details.is_empty() {
        let _ = write!(description, "\n**Details:** {details}");
    }
    if let Some(ip) = reporter_ip {
        let _ = write!(description, "\n**Reporter (hashed):** {ip}");
    }

    let payload = serde_json::json!({
        "embeds": [{
            "title": "New Content Report",
            "description": description,
            "color": 0x00ED_4245,
        }]
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("webhook request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("webhook returned {status}: {body}"));
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "pipeline fns thread established context (state, ids, tokens); bundling params churns callers for no behavior gain"
)]
async fn send_smtp_email(
    host: &str,
    port: u16,
    username: Option<&str>,
    password: Option<&str>,
    sender: &str,
    recipient: &str,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    use lettre::{
        Message, SmtpTransport, Transport, message::header::ContentType,
        transport::smtp::authentication::Credentials,
    };

    let email = Message::builder()
        .from(
            sender
                .parse()
                .map_err(|e| format!("invalid sender email: {e}"))?,
        )
        .to(recipient
            .parse()
            .map_err(|e| format!("invalid recipient email: {e}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| format!("failed to build email: {e}"))?;

    let mut builder = SmtpTransport::relay(host)
        .map_err(|e| format!("SMTP relay error: {e}"))?
        .port(port);

    if let (Some(user), Some(pass)) = (username, password) {
        builder = builder.credentials(Credentials::new(user.to_string(), pass.to_string()));
    }

    let mailer = builder.build();
    tokio::task::spawn_blocking(move || {
        mailer
            .send(&email)
            .map_err(|e| format!("SMTP send error: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("SMTP task panicked: {e}"))?
}
