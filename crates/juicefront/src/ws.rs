use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    response::Response,
};
use tokio::{io::AsyncWriteExt, net::TcpStream};

use crate::{config::Config, proxy::clean_headers, state::AppState};

pub async fn device_ws_proxy(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    req: Request<Body>,
) -> Response<Body> {
    let is_websocket = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if req.method() != axum::http::Method::GET || !is_websocket {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("expected a websocket upgrade"))
            .unwrap_or_default();
    }

    let Some((upstream_host, upstream_port)) =
        Config::upstream_host_port(&state.config.juiceback_url)
    else {
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::from("Proxy error: upstream unavailable"))
            .unwrap_or_default();
    };

    let request_line = format!(
        "GET {} HTTP/1.1\r\n",
        req.uri()
            .path_and_query()
            .map_or("/api/device/ws", axum::http::uri::PathAndQuery::as_str)
    );
    let mut header_block = Vec::new();
    for (name, value) in clean_headers(req.headers()) {
        header_block.extend_from_slice(name.as_str().as_bytes());
        header_block.extend_from_slice(b": ");
        header_block.extend_from_slice(value.as_bytes());
        header_block.extend_from_slice(b"\r\n");
    }
    header_block.extend_from_slice(b"host: ");
    header_block.extend_from_slice(upstream_host.as_bytes());
    header_block.extend_from_slice(b"\r\n");
    header_block.extend_from_slice(b"x-forwarded-for: ");
    header_block.extend_from_slice(peer.ip().to_string().as_bytes());
    header_block.extend_from_slice(b"\r\n\r\n");

    let mut raw_request = request_line.into_bytes();
    raw_request.extend_from_slice(&header_block);

    let shutdown = Arc::clone(&state.shutdown);
    tokio::spawn(async move {
        if let Err(err) =
            pipe_upgrade(req, &upstream_host, upstream_port, &raw_request, shutdown).await
        {
            tracing::debug!(error = %format!("{err:#}"), "device ws proxy ended");
        }
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .body(Body::empty())
        .unwrap_or_default()
}

async fn pipe_upgrade(
    mut req: Request<Body>,
    upstream_host: &str,
    upstream_port: u16,
    raw_request: &[u8],
    shutdown: Arc<tokio::sync::Notify>,
) -> anyhow::Result<()> {
    let mut downstream = hyper_util::rt::TokioIo::new(
        hyper::upgrade::on(&mut req)
            .await
            .map_err(|err| anyhow::anyhow!("upgrade failed: {err}"))?,
    );
    let mut upstream = TcpStream::connect((upstream_host, upstream_port))
        .await
        .map_err(|err| anyhow::anyhow!("dial upstream failed: {err}"))?;
    upstream
        .write_all(raw_request)
        .await
        .map_err(|err| anyhow::anyhow!("upstream write failed: {err}"))?;
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut downstream, &mut upstream) => {
            result.map_err(|err| anyhow::anyhow!("proxy copy failed: {err}"))?;
        }
        () = shutdown.notified() => {}
    }
    Ok(())
}
