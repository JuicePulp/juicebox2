use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};

use crate::state::AppState;

pub fn live_reload_enabled() -> bool {
    juiceutils::config::optional_secret("JUICEFRONT_LIVE").map_or(cfg!(debug_assertions), |value| {
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}

pub async fn live_handler(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let boot_id = state.boot_id.clone();
    let shutdown = std::sync::Arc::clone(&state.shutdown);
    let stop = async move { shutdown.notified().await };
    let stream = async_stream::stream! {
        yield Ok(Event::default().event("boot").data(boot_id));
        std::future::pending::<()>().await;
    };
    Sse::new(futures::StreamExt::take_until(stream, stop)).keep_alive(KeepAlive::default())
}
