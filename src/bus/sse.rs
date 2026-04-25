use crate::http::AppState;
use axum::{extract::State, response::sse::{Event as SseEvent, KeepAlive, Sse}};
use futures::stream::{self, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.bus.tx.subscribe();
    let s = BroadcastStream::new(rx).filter_map(|res| async move {
        match res {
            Ok(ev) => Some(Ok(SseEvent::default().json_data(ev).unwrap())),
            Err(_) => None,
        }
    });
    let initial = stream::once(async { Ok(SseEvent::default().comment("connected")) });
    Sse::new(initial.chain(s)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
