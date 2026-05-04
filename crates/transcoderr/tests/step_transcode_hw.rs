use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::hw::{
    devices::{Accel, Device, HwCaps},
    semaphores::DeviceRegistry,
};
use transcoderr::steps::{transcode::TranscodeStep, Step, StepProgress};

#[tokio::test]
async fn cpu_path_when_no_hw_preferred() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("in.mkv");
    make_testsrc_mkv(&p, 2).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    let reg = DeviceRegistry::from_caps(&HwCaps::default());
    let step = TranscodeStep { hw: reg };

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("codec".into(), json!("x264"));
    with.insert("crf".into(), json!(30));
    with.insert("preset".into(), json!("ultrafast"));

    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    step.execute(&with, &mut ctx, &mut cb).await.unwrap();
    let out = ctx.steps["transcode"]["output_path"].as_str().unwrap();
    assert!(
        std::path::Path::new(out).exists(),
        "output file should exist"
    );
    assert!(
        ctx.steps["transcode"]["hw"].is_null(),
        "hw should be null when no hw block given"
    );
}

#[tokio::test]
async fn fallback_to_cpu_when_no_gpu_slot() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("in.mkv");
    make_testsrc_mkv(&p, 1).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());

    // Caps says nvenc exists with limit=1, but we'll exhaust it before calling.
    let caps = HwCaps {
        probed_at: 0,
        ffmpeg_version: None,
        encoders: vec![],
        devices: vec![Device {
            accel: Accel::Nvenc,
            index: 0,
            name: "fake".into(),
            max_concurrent: 1,
        }],
    };
    let reg = DeviceRegistry::from_caps(&caps);
    // Pre-acquire to exhaust.
    let _hold = reg.acquire_preferred(&[Accel::Nvenc]).await.unwrap();
    let step = TranscodeStep { hw: reg };

    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("codec".into(), json!("x264"));
    with.insert("crf".into(), json!(30));
    with.insert("preset".into(), json!("ultrafast"));
    with.insert(
        "hw".into(),
        json!({ "prefer": ["nvenc"], "fallback": "cpu" }),
    );

    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    step.execute(&with, &mut ctx, &mut cb).await.unwrap();
    assert!(
        ctx.steps["transcode"]["output_path"].is_string(),
        "should have produced output via CPU fallback"
    );
    // Verify hw_unavailable marker was emitted.
    let has_hw_unavailable = events.iter().any(|e| {
        matches!(
            e,
            StepProgress::Marker { kind, .. } if kind == "hw_unavailable"
        )
    });
    assert!(has_hw_unavailable, "expected hw_unavailable marker event");
}

/// Drive through the engine and assert hw_unavailable appears in run_events.
#[tokio::test]
async fn engine_records_hw_unavailable_event() {
    use transcoderr::{
        db,
        flow::{parse_flow, Engine},
    };

    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();

    // Build caps with nvenc limit=1 and exhaust it.
    let caps = HwCaps {
        probed_at: 0,
        ffmpeg_version: None,
        encoders: vec![],
        devices: vec![Device {
            accel: Accel::Nvenc,
            index: 0,
            name: "fake".into(),
            max_concurrent: 1,
        }],
    };
    let reg = DeviceRegistry::from_caps(&caps);
    let _hold = reg.acquire_preferred(&[Accel::Nvenc]).await.unwrap();

    // Init step registry with this registry so transcode step is aware of it.
    transcoderr::steps::registry::init(
        pool.clone(),
        reg,
        std::sync::Arc::new(transcoderr::ffmpeg_caps::FfmpegCaps::default()),
        vec![],
    )
    .await;

    // Make a source file.
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 1).await.unwrap();

    let yaml = r#"
name: hw_test
triggers: [{ radarr: [downloaded] }]
steps:
  - id: transcode
    use: transcode
    with:
      codec: x264
      crf: 30
      preset: ultrafast
      hw:
        prefer: [nvenc]
        fallback: cpu
"#
    .to_string();
    let flow = parse_flow(&yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "hw_test", &yaml, &flow)
        .await
        .unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", src.to_str().unwrap(), "{}")
        .await
        .unwrap();
    let _ = db::jobs::claim_next(&pool).await.unwrap().unwrap();

    let ctx = Context::for_file(src.to_str().unwrap());
    let bus = transcoderr::bus::Bus::default();
    let outcome = Engine::new(pool.clone(), bus, dir.path().to_path_buf())
        .run(&flow, job_id, ctx)
        .await
        .unwrap();
    assert_eq!(
        outcome.status, "completed",
        "job should complete via CPU fallback"
    );

    // Allow spawned db writes to flush.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Assert hw_unavailable event exists in run_events.
    let evt: Option<(String,)> = sqlx::query_as(
        "SELECT kind FROM run_events WHERE job_id = ? AND kind = 'hw_unavailable' LIMIT 1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(evt.is_some(), "expected hw_unavailable row in run_events");
}
