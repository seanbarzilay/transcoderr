use crate::http::AppState;
use axum::{
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use futures::stream::{self, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.bus.tx.subscribe();

    // Subscribe BEFORE we query the DB so we can't miss an event the worker
    // emits between the snapshot and our subscription. Then send a fresh Queue
    // snapshot up front so the Dashboard's tiles populate on connect rather
    // than waiting for the next worker tick.
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'pending'")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);
    let running: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'running'")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);
    let snapshot = SseEvent::default()
        .json_data(crate::bus::Event::Queue { pending, running })
        .unwrap();

    let s = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(ev) => Some(Ok(SseEvent::default().json_data(ev).unwrap())),
            Err(_) => None,
        }
    });
    let connected = stream::once(async { Ok(SseEvent::default().comment("connected")) });
    let initial = stream::once(async move { Ok::<_, Infallible>(snapshot) });
    Sse::new(connected.chain(initial).chain(s))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
