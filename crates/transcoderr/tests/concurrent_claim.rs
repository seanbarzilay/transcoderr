//! Regression: with runs.max_concurrent > 1, multiple claim loops race
//! against claim_next. A single job must only be claimed once. The
//! previous implementation didn't check rows_affected on the UPDATE,
//! so two loops seeing the same pending row in their snapshots would
//! both return Ok(Some(job)) — the same job — and run it twice in
//! parallel.

use tempfile::tempdir;
use transcoderr::db;
use transcoderr::flow::parse_flow;

#[tokio::test]
async fn concurrent_claim_returns_a_single_job_once() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();

    let yaml = "name: t\ntriggers: [{ radarr: [downloaded] }]\nsteps:\n  - use: probe\n";
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "t", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", "/tmp/x.mkv", "{}")
        .await
        .unwrap();

    // Fire 8 claim_next calls concurrently against a single pending job.
    // Exactly one should return Some(job); the rest must return None.
    let pool2 = pool.clone();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let p = pool2.clone();
            tokio::spawn(async move { db::jobs::claim_next(&p).await })
        })
        .collect();

    let mut claimed = 0;
    let mut nones = 0;
    for h in handles {
        match h.await.unwrap().unwrap() {
            Some(j) => {
                assert_eq!(j.id, job_id);
                claimed += 1;
            }
            None => nones += 1,
        }
    }
    assert_eq!(
        claimed, 1,
        "exactly one worker must claim the job (got {claimed})"
    );
    assert_eq!(nones, 7, "the other 7 workers must see None (got {nones})");

    // The job is now in 'running' state and a fresh claim_next sees nothing.
    assert!(db::jobs::claim_next(&pool).await.unwrap().is_none());
}
