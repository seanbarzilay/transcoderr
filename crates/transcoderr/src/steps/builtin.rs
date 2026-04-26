use crate::hw::semaphores::DeviceRegistry;
use crate::steps::{
    audio_ensure::AudioEnsureStep,
    copy_step::CopyStep,
    delete_step::DeleteStep,
    extract_subs::ExtractSubsStep,
    iso_extract::IsoExtractStep,
    move_step::MoveStep,
    notify::NotifyStep,
    output::OutputStep,
    plan_execute::PlanExecuteStep,
    plan_steps::{
        PlanAudioEnsureStep, PlanContainerStep, PlanDropCoverArtStep, PlanDropDataStep,
        PlanDropUnsupportedSubsStep, PlanInitStep, PlanTolerateErrorsStep, PlanVideoEncodeStep,
        PlanVideoTonemapStep,
    },
    probe::ProbeStep,
    remux::RemuxStep,
    shell::ShellStep,
    strip_tracks::StripTracksStep,
    transcode::TranscodeStep,
    verify_playable::VerifyPlayableStep,
    Step,
};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;

pub fn register_all(
    map: &mut HashMap<String, Arc<dyn Step>>,
    pool: SqlitePool,
    hw: DeviceRegistry,
    ffmpeg_caps: std::sync::Arc<crate::ffmpeg_caps::FfmpegCaps>,
) {
    map.insert("probe".into(), Arc::new(ProbeStep));
    map.insert("transcode".into(), Arc::new(TranscodeStep { hw: hw.clone() }));
    map.insert("output".into(), Arc::new(OutputStep));
    map.insert("verify.playable".into(), Arc::new(VerifyPlayableStep));
    map.insert("remux".into(), Arc::new(RemuxStep));
    map.insert("extract.subs".into(), Arc::new(ExtractSubsStep));
    map.insert("strip.tracks".into(), Arc::new(StripTracksStep));
    map.insert("audio.ensure".into(), Arc::new(AudioEnsureStep));

    // Preprocessing: demux Blu-ray ISOs into a sibling .m2ts before the plan
    // pipeline probes the input. No-op for non-ISO inputs.
    map.insert("iso.extract".into(), Arc::new(IsoExtractStep));

    // New plan-then-execute pipeline. Mutator steps are pure (no ffmpeg), the
    // executor materializes everything into one ffmpeg pass.
    map.insert("plan.init".into(), Arc::new(PlanInitStep));
    map.insert("plan.input.tolerate_errors".into(), Arc::new(PlanTolerateErrorsStep));
    map.insert("plan.streams.drop_cover_art".into(), Arc::new(PlanDropCoverArtStep));
    map.insert("plan.streams.drop_data".into(), Arc::new(PlanDropDataStep));
    map.insert("plan.subs.drop_unsupported".into(), Arc::new(PlanDropUnsupportedSubsStep));
    map.insert("plan.container".into(), Arc::new(PlanContainerStep));
    map.insert("plan.video.encode".into(), Arc::new(PlanVideoEncodeStep));
    map.insert("plan.video.tonemap".into(), Arc::new(PlanVideoTonemapStep));
    map.insert("plan.audio.ensure".into(), Arc::new(PlanAudioEnsureStep));
    map.insert(
        "plan.execute".into(),
        Arc::new(PlanExecuteStep {
            hw: hw.clone(),
            ffmpeg_caps: ffmpeg_caps.clone(),
        }),
    );
    map.insert("move".into(), Arc::new(MoveStep));
    map.insert("copy".into(), Arc::new(CopyStep));
    map.insert("delete".into(), Arc::new(DeleteStep));
    map.insert("shell".into(), Arc::new(ShellStep));
    map.insert("notify".into(), Arc::new(NotifyStep { pool: pool.clone() }));
}
