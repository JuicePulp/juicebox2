use std::{
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::sse::{Event, Sse},
};
use futures::StreamExt;
use rusqlite::OptionalExtension;
use serde::Serialize;
use tokio::sync::broadcast;
use utoipa::ToSchema;

use crate::{
    auth::DeviceAuth,
    error::AppError,
    routes::UserId,
    state::{AppState, ConnectedDevice, PresenceEvent},
    utils::ClientIp,
};

struct PresenceIpGuard {
    state: Arc<AppState>,
    ip: std::net::IpAddr,
    user_id: String,
}

impl Drop for PresenceIpGuard {
    fn drop(&mut self) {
        if let Some(mut count) = self.state.presence_by_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                drop(count);
                self.state.presence_by_ip.remove(&self.ip);
            }
        }
        if !self.state.connected_devices.contains_key(&self.user_id)
            && self
                .state
                .presence_listeners
                .get(&self.user_id)
                .is_some_and(|sender| sender.receiver_count() <= 1)
        {
            self.state.presence_listeners.remove(&self.user_id);
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct DeviceListResponse {
    pub devices: Vec<PresenceDevice>,
}

#[derive(Serialize, ToSchema)]
pub struct PresenceDevice {
    pub device_id: String,
    pub device_name: String,
}

/// Snapshot a user's connected devices for API responses. Devices whose
/// lock is held are skipped rather than blocking the listing task.
fn snapshot_device_list(state: &AppState, user_id: &str) -> Vec<PresenceDevice> {
    let guard = state.connected_devices.get(user_id);
    guard
        .map(|d| {
            d.value()
                .iter()
                .filter_map(|d| {
                    d.try_lock().ok().map(|inner| PresenceDevice {
                        device_id: inner.device_id.clone(),
                        device_name: inner.device_name.clone(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

// Device WebSocket endpoint

/// GET /api/device/ws where juicebox-plus connects, auth via Bearer.
pub async fn device_ws_handler(
    State(state): State<Arc<AppState>>,
    DeviceAuth(claims): DeviceAuth,
    ws: WebSocketUpgrade,
) -> Result<axum::response::Response, AppError> {
    let device_id = claims.sub;
    let user_id = claims.user_id;
    let expires_at = claims.exp as i64;
    Ok(ws.on_upgrade(move |socket| {
        handle_device_socket(socket, state, device_id, user_id, expires_at)
    }))
}

async fn handle_device_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    device_id: String,
    user_id: String,
    expires_at: i64,
) {
    let device_name = {
        let did = device_id.clone();
        state
            .db_call("get_device_name", move |db| {
                let mut stmt = db.prepare("SELECT device_name FROM devices WHERE id = ?1")?;
                let name = stmt
                    .query_row(rusqlite::params![did], |row| row.get::<_, String>(0))
                    .optional()?;
                Ok(name.unwrap_or_else(|| "Unknown".into()))
            })
            .await
            .unwrap_or_else(|_| "Unknown".into())
    };

    let auth_ok = serde_json::json!({
        "type": "auth_ok",
        "device_id": device_id,
        "device_name": device_name,
    });
    if socket
        .send(Message::Text(auth_ok.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    {
        let did = device_id.clone();
        let _ = state
            .db_call("update_device_seen", move |db| {
                db.execute(
                    "UPDATE devices SET last_seen_at = ?1 WHERE id = ?2",
                    rusqlite::params![chrono::Utc::now().timestamp(), did],
                )
            })
            .await;
    }

    let tx = state
        .presence_listeners
        .entry(user_id.clone())
        .or_insert_with(|| {
            let (tx, _) = broadcast::channel(64);
            tx
        })
        .clone();

    let _ = tx.send(PresenceEvent::DeviceConnected {
        device_id: device_id.clone(),
        device_name: device_name.clone(),
    });

    let (device_tx, mut device_rx) = tokio::sync::mpsc::channel::<String>(32);
    let device = Arc::new(tokio::sync::Mutex::new(ConnectedDevice {
        device_id: device_id.clone(),
        device_name: device_name.clone(),
        sender: device_tx,
        connected_at: Instant::now(),
        last_heartbeat: Instant::now(),
    }));

    state
        .connected_devices
        .entry(user_id.clone())
        .or_default()
        .push(Arc::clone(&device));

    let devices = snapshot_device_list(&state, &user_id);

    let _ = socket
        .send(Message::Text(
            serde_json::json!({
                "type": "device_list",
                "devices": devices,
            })
            .to_string()
            .into(),
        ))
        .await;

    let mut last_heartbeat = Instant::now();
    let stale_threshold = Duration::from_secs(90);
    // `last_seen_at` rewrites are coalesced: heartbeats still ack every
    // message, but the DB write happens at most once per interval.
    let mut last_seen_write = Instant::now();

    loop {
        tokio::select! {
            // Incoming message from juicebox-plus
            msg = socket.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let text_str: &str = &text;
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text_str) {
                            match val.get("type").and_then(|t| t.as_str()) {
                                Some("heartbeat") => {
                                    last_heartbeat = Instant::now();
                                    if last_seen_write.elapsed()
                                        >= Duration::from_secs(
                                            crate::constants::DEVICE_SEEN_TOUCH_INTERVAL_SECS,
                                        )
                                    {
                                        let did = device_id.clone();
                                        let _ = state.db_call("update_heartbeat", move |db| {
                                            db.execute(
                                                "UPDATE devices SET last_seen_at = ?1 WHERE id = ?2",
                                                rusqlite::params![chrono::Utc::now().timestamp(), did],
                                            )
                                        }).await;
                                        last_seen_write = Instant::now();
                                    }
                                    let _ = socket.send(Message::Text(
                                        serde_json::json!({"type":"heartbeat_ack"}).to_string().into()
                                    )).await;
                                }
                                Some("ping") => {
                                    let _ = socket.send(Message::Text(
                                        serde_json::json!({"type":"ping_response"}).to_string().into()
                                    )).await;
                                }
                                Some("ping_response") => {
                                    let _ = tx.send(PresenceEvent::PingResponse {
                                        device_id: device_id.clone(),
                                    });
                                }
                                Some("upload_progress") => {
                                    let _ = tx.send(PresenceEvent::DeviceConnected {
                                        device_id: device_id.clone(),
                                        device_name: device_name.clone(),
                                    });
                                }
                                Some("upload_complete") | Some("upload_error") => {
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }

            // Outgoing messages (e.g., upload_request from juiceback)
            outgoing = device_rx.recv() => {
                if let Some(text) = outgoing {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
            }

            // Stale connection check
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                let did = device_id.clone();
                let uid = user_id.clone();
                let still_paired = state.db_call("revalidate_device_ws", move |db| {
                    crate::db::device_belongs_to_user(db, &did, &uid)
                }).await.unwrap_or(false);
                if chrono::Utc::now().timestamp() >= expires_at || !still_paired {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                if last_heartbeat.elapsed() > stale_threshold {
                    tracing::warn!("Device {} timed out (no heartbeat for {:?})", device_id, last_heartbeat.elapsed());
                    break;
                }
            }
        }
    }

    let _ = tx.send(PresenceEvent::DeviceDisconnected {
        device_id: device_id.clone(),
    });

    if let Some(mut devices) = state.connected_devices.get_mut(&user_id) {
        devices.retain(|d| {
            if let Ok(inner) = d.try_lock() {
                inner.device_id != device_id
            } else {
                true
            }
        });
        if devices.is_empty() {
            drop(devices);
            state.connected_devices.remove(&user_id);
            if state
                .presence_listeners
                .get(&user_id)
                .is_some_and(|sender| sender.receiver_count() == 0)
            {
                state.presence_listeners.remove(&user_id);
            }
        }
    }

    tracing::info!("Device {device_id} disconnected");
}

// Device pairing status check (used by juicebox-plus handshake)

#[utoipa::path(
    get,
    path = "/api/device/status",
    params(
        ("token" = String, Query, description = "Device JWT (alternative to Authorization header)"),
    ),
    responses(
        (status = 200, description = "Device is still paired", body = serde_json::Value),
        (status = 401, description = "Invalid token or device no longer paired"),
    ),
    tag = "Devices",
)]
/// GET /api/device/status returns 200 if still paired and 401 if not
pub async fn device_status_handler(
    DeviceAuth(_claims): DeviceAuth,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({ "paired": true })))
}

// SSE presence stream (for website)

#[utoipa::path(
    get,
    path = "/api/presence",
    responses(
        (status = 200, description = "SSE stream of device connect/disconnect/ping events"),
    ),
    tag = "Devices",
)]
/// `GET /api/presence` - SSE stream of device connect/disconnect events.
pub async fn presence_sse_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
    axum::extract::Extension(client_ip): axum::extract::Extension<ClientIp>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let permit = state
        .presence_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests("too many presence streams".into()))?;
    {
        let mut count = state.presence_by_ip.entry(client_ip.0).or_default();
        if *count >= crate::constants::MAX_SSE_CONNECTIONS_PER_IP as usize {
            return Err(AppError::TooManyRequests(
                "too many presence streams from this address".into(),
            ));
        }
        *count += 1;
    }
    let ip_guard = PresenceIpGuard {
        state: Arc::clone(&state),
        ip: client_ip.0,
        user_id: user_id.clone(),
    };
    let tx = state
        .presence_listeners
        .entry(user_id.clone())
        .or_insert_with(|| {
            let (tx, _) = broadcast::channel(64);
            tx
        })
        .clone();

    let user_id_clone = user_id.clone();
    let state_clone = Arc::clone(&state);

    let stream = async_stream::stream! {
        let _permit = permit;
        let _ip_guard = ip_guard;
        let devices = snapshot_device_list(&state_clone, &user_id_clone);

        let initial = serde_json::json!({
            "type": "device_list",
            "devices": devices,
        });
        yield Ok(Event::default().data(initial.to_string()));

        let mut rx = tx.subscribe();
        while let Ok(event) = rx.recv().await {
            let data = match event {
                PresenceEvent::DeviceConnected { device_id, device_name } => {
                    serde_json::json!({
                        "type": "device_connected",
                        "device_id": device_id,
                        "device_name": device_name,
                    })
                }
                PresenceEvent::DeviceDisconnected { device_id } => {
                    serde_json::json!({
                        "type": "device_disconnected",
                        "device_id": device_id,
                    })
                }
                PresenceEvent::PingResponse { device_id } => {
                    serde_json::json!({
                        "type": "ping_response",
                        "device_id": device_id,
                    })
                }
            };
            yield Ok(Event::default().data(data.to_string()));
        }
    };

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("ping"),
    ))
}

// Ping device

#[utoipa::path(
    post,
    path = "/api/device/ping",
    responses(
        (status = 200, description = "Ping sent to connected devices", body = serde_json::Value),
    ),
    tag = "Devices",
)]
pub async fn ping_device_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
) -> Result<Json<serde_json::Value>, AppError> {
    let devices = state
        .connected_devices
        .get(&user_id)
        .map(|d| d.value().clone())
        .unwrap_or_default();

    if devices.is_empty() {
        return Ok(Json(
            serde_json::json!({"ok": false, "error": "no device connected"}),
        ));
    }

    let ping_msg = serde_json::json!({"type": "ping"}).to_string();
    let mut sent = 0;
    for device in &devices {
        if let Ok(inner) = device.try_lock() {
            if inner.sender.send(ping_msg.clone()).await.is_ok() {
                sent += 1;
            }
        }
    }

    Ok(Json(serde_json::json!({"ok": true, "sent": sent})))
}

// Device list REST endpoint

#[utoipa::path(
    get,
    path = "/api/device/connected",
    responses(
        (status = 200, description = "List of currently connected devices", body = DeviceListResponse),
    ),
    tag = "Devices",
)]
pub async fn list_connected_devices_handler(
    State(state): State<Arc<AppState>>,
    UserId(user_id): UserId,
) -> Result<Json<DeviceListResponse>, AppError> {
    let devices = snapshot_device_list(&state, &user_id);

    Ok(Json(DeviceListResponse { devices }))
}
