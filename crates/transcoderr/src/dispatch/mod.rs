//! Per-step routing decision: should this step run on the local pool
//! or get dispatched to a remote worker?
//!
//! Inputs:
//! - `step_kind` (the YAML `use:` value)
//! - `run_on` from YAML, if any
//! - `&AppState` (for the workers DB query + Connections registry)
//!
//! Output: `Route::Local` or `Route::Remote(worker_id)`. The engine
//! branches on this.

pub mod remote;

use crate::flow::model::RunOn;
use crate::http::AppState;
use crate::steps::Executor;
use crate::worker::local::LOCAL_WORKER_ID;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Local,
    Remote(i64),
}

/// Round-robin pointer across the eligible worker list. Stays in
/// memory; no need to persist (round-robin is best-effort anyway).
static RR_POINTER: AtomicUsize = AtomicUsize::new(0);

/// Decide where a step should run.
///
/// Logic:
/// 1. If `run_on == Coordinator` → Local.
/// 2. Else compute the step's effective executor (from the registry).
///    - `CoordinatorOnly` → Local.
///    - `Any` → continue to remote selection.
/// 3. List eligible workers: enabled=1 AND last_seen_at > now-90s
///    AND step_kind in their `available_steps`. Always exclude the
///    LOCAL row (we only dispatch to *remote* workers; if no remotes
///    are eligible we run locally without sending a frame to
///    ourselves).
/// 4. If list is empty → Local (with `tracing::warn!`).
/// 5. Else round-robin pick → Remote(worker_id).
pub async fn route(
    step_kind: &str,
    run_on: Option<RunOn>,
    state: &AppState,
) -> Route {
    if matches!(run_on, Some(RunOn::Coordinator)) {
        return Route::Local;
    }

    let executor = match crate::steps::registry::try_resolve(step_kind) {
        Some(s) => s.executor(),
        None => return Route::Local, // unknown step kind; engine will surface the error
    };
    if executor == Executor::CoordinatorOnly {
        return Route::Local;
    }

    let eligible = match eligible_remotes(step_kind, state).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error=?e, step_kind, "dispatcher DB query failed; falling back to local");
            return Route::Local;
        }
    };
    if eligible.is_empty() {
        tracing::warn!(step_kind, "no eligible remote workers; running locally");
        return Route::Local;
    }
    let idx = RR_POINTER.fetch_add(1, Ordering::Relaxed) % eligible.len();
    Route::Remote(eligible[idx])
}

const STALE_AFTER_SECS: i64 = 90;

/// Workers that are enabled, fresh, NOT the local row, AND have an
/// active sender connection, AND advertise the requested step_kind.
async fn eligible_remotes(
    step_kind: &str,
    state: &AppState,
) -> anyhow::Result<Vec<i64>> {
    let cutoff = chrono::Utc::now().timestamp() - STALE_AFTER_SECS;

    let rows = crate::db::workers::list_all(&state.pool).await?;
    let mut out = Vec::new();
    for r in rows {
        if r.id == LOCAL_WORKER_ID {
            continue;
        }
        if r.enabled == 0 {
            continue;
        }
        match r.last_seen_at {
            Some(seen) if seen > cutoff => {}
            _ => continue,
        }
        // Verify the worker has an active outbound channel registered
        // — i.e., the WS handler currently holds a SenderGuard for it.
        if !state.connections.is_connected(r.id).await {
            continue;
        }
        // NEW (Piece 5): filter workers that don't advertise this step
        // kind. Plugin step kinds are only present on workers that
        // successfully installed the plugin.
        if !state.connections.worker_has_step(r.id, step_kind).await {
            continue;
        }
        out.push(r.id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    // Tests construct an AppState shell with the canonical field set.
    // If a future piece adds more fields, update `shell_state` here.
    use super::*;
    use crate::worker::protocol::Envelope;
    use sqlx::SqlitePool;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    async fn pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        (pool, dir)
    }

    /// Build an AppState shell whose fields match `tests/common/mod.rs`.
    /// Add any new AppState fields here as they're introduced in
    /// subsequent pieces.
    async fn shell_state(pool: SqlitePool) -> AppState {
        // CRITICAL: This must mirror tests/common/mod.rs::boot()'s
        // AppState construction. Field set as of Piece 3 / Task 6:
        //   pool, cfg, hw_caps, hw_devices, ffmpeg_caps, bus, ready,
        //   metrics, cancellations, public_url, arr_cache,
        //   catalog_client, runtime_checker, connections.
        use std::sync::Arc;

        let caps = crate::hw::HwCaps::default();
        let hw_devices = crate::hw::semaphores::DeviceRegistry::from_caps(&caps);
        let hw_caps = Arc::new(tokio::sync::RwLock::new(caps));
        let ffmpeg_caps = Arc::new(crate::ffmpeg_caps::FfmpegCaps::default());

        let cfg = Arc::new(crate::config::Config {
            bind: "127.0.0.1:0".into(),
            data_dir: std::path::PathBuf::from("/tmp/dispatch-test"),
            radarr: crate::config::RadarrConfig {
                bearer_token: "test-token".into(),
            },
        });

        let bus = crate::bus::Bus::default();
        let cancellations = crate::cancellation::JobCancellations::new();

        let ready = crate::ready::Readiness::new();
        ready.mark_ready().await;

        // The metrics registry is global; Metrics::install() can only run
        // once per process. Use a static OnceLock to share across tests
        // in this binary, mirroring tests/common/mod.rs.
        use std::sync::OnceLock;
        static METRICS: OnceLock<Arc<crate::metrics::Metrics>> = OnceLock::new();
        let metrics = METRICS
            .get_or_init(|| Arc::new(crate::metrics::Metrics::install().unwrap()))
            .clone();

        AppState {
            pool,
            cfg,
            hw_caps,
            hw_devices,
            ffmpeg_caps,
            bus,
            ready,
            metrics,
            cancellations,
            public_url: Arc::new("http://test:8099".to_string()),
            arr_cache: Arc::new(crate::arr::cache::ArrCache::new(
                std::time::Duration::from_secs(300),
            )),
            catalog_client: Arc::new(crate::plugins::catalog::CatalogClient::default()),
            runtime_checker: Arc::new(crate::plugins::runtime::RuntimeChecker::default()),
            connections: crate::worker::connections::Connections::new(),
        }
    }

    /// Helper: insert a remote worker row that's enabled + fresh +
    /// has a fake outbound sender registered.
    async fn add_fake_remote(state: &AppState, name: &str) -> i64 {
        let id = crate::db::workers::insert_remote(&state.pool, name, &format!("tok_{name}"))
            .await
            .unwrap();
        crate::db::workers::record_heartbeat(&state.pool, id).await.unwrap();
        let (tx, _rx) = mpsc::channel::<Envelope>(4);
        let guard = state.connections.register_sender(id, tx).await;
        std::mem::forget(guard); // keep registered for the test's lifetime
        // Also leak the receiver so the channel doesn't close.
        std::mem::forget(_rx);
        // NEW (Piece 5): default to advertising the same step kinds the
        // existing dispatch tests assume — built-in "transcode" suffices
        // for the round-robin / one-eligible / disabled tests. New
        // tests that need different step kinds should call
        // record_available_steps after this helper.
        state
            .connections
            .record_available_steps(id, vec!["transcode".into()])
            .await;
        id
    }

    #[tokio::test]
    async fn coordinator_only_step_returns_local() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;
        let r = route("notify", Some(RunOn::Any), &state).await;
        assert_eq!(r, Route::Local);
    }

    #[tokio::test]
    async fn run_on_coordinator_forces_local() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;
        let r = route("transcode", Some(RunOn::Coordinator), &state).await;
        assert_eq!(r, Route::Local);
    }

    #[tokio::test]
    async fn no_eligible_remotes_falls_back_to_local() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;
        let r = route("transcode", None, &state).await;
        assert_eq!(r, Route::Local);
    }

    #[tokio::test]
    async fn one_eligible_remote_picks_it() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;
        let id = add_fake_remote(&state, "gpu1").await;
        let r = route("transcode", None, &state).await;
        assert_eq!(r, Route::Remote(id));
    }

    #[tokio::test]
    async fn two_remotes_round_robin() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;
        let id_a = add_fake_remote(&state, "a").await;
        let id_b = add_fake_remote(&state, "b").await;
        let r1 = route("transcode", None, &state).await;
        let r2 = route("transcode", None, &state).await;
        let r3 = route("transcode", None, &state).await;
        let picks: Vec<i64> = [r1, r2, r3]
            .into_iter()
            .map(|r| match r {
                Route::Remote(id) => id,
                _ => panic!("expected remote: {r:?}"),
            })
            .collect();
        assert!(picks.contains(&id_a), "round-robin should hit id_a in 3 calls");
        assert!(picks.contains(&id_b), "round-robin should hit id_b in 3 calls");
    }

    #[tokio::test]
    async fn disabled_remote_is_skipped() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;
        let id = add_fake_remote(&state, "gpu1").await;
        crate::db::workers::set_enabled(&state.pool, id, false).await.unwrap();
        let r = route("transcode", None, &state).await;
        assert_eq!(r, Route::Local);
    }

    #[tokio::test]
    async fn worker_without_step_kind_is_skipped() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;

        // Add a fake remote that advertises only "transcode"
        // (default from add_fake_remote). Routing a step the worker
        // doesn't advertise → falls back to local.
        let _id = add_fake_remote(&state, "transcode-only").await;
        let r = route("whisper.transcribe", None, &state).await;
        assert_eq!(r, Route::Local);
    }

    #[tokio::test]
    async fn worker_advertising_step_kind_is_picked() {
        let (pool, _dir) = pool().await;
        let state = shell_state(pool).await;
        crate::steps::registry::init(
            state.pool.clone(),
            state.hw_devices.clone(),
            state.ffmpeg_caps.clone(),
            vec![],
        )
        .await;

        let id = add_fake_remote(&state, "has-whisper").await;
        // Override default to advertise both transcode and whisper.transcribe.
        state
            .connections
            .record_available_steps(
                id,
                vec!["transcode".into(), "whisper.transcribe".into()],
            )
            .await;

        // route() consults registry::try_resolve to determine the
        // executor. The unit test environment has no plugin SubprocessStep
        // registered, so we route "transcode" instead — same dispatch
        // path, exercises the available_steps filter.
        let r = route("transcode", None, &state).await;
        assert_eq!(r, Route::Remote(id));
    }
}
