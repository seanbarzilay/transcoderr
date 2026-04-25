use crate::hw::semaphores::DeviceRegistry;
use crate::steps::{
    audio_ensure::AudioEnsureStep,
    copy_step::CopyStep,
    delete_step::DeleteStep,
    extract_subs::ExtractSubsStep,
    move_step::MoveStep,
    notify::NotifyStep,
    output::OutputStep,
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
) {
    map.insert("probe".into(), Arc::new(ProbeStep));
    map.insert("transcode".into(), Arc::new(TranscodeStep { hw }));
    map.insert("output".into(), Arc::new(OutputStep));
    map.insert("verify.playable".into(), Arc::new(VerifyPlayableStep));
    map.insert("remux".into(), Arc::new(RemuxStep));
    map.insert("extract.subs".into(), Arc::new(ExtractSubsStep));
    map.insert("strip.tracks".into(), Arc::new(StripTracksStep));
    map.insert("audio.ensure".into(), Arc::new(AudioEnsureStep));
    map.insert("move".into(), Arc::new(MoveStep));
    map.insert("copy".into(), Arc::new(CopyStep));
    map.insert("delete".into(), Arc::new(DeleteStep));
    map.insert("shell".into(), Arc::new(ShellStep));
    map.insert("notify".into(), Arc::new(NotifyStep { pool: pool.clone() }));
}
