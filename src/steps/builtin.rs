use crate::steps::{output::OutputStep, probe::ProbeStep, transcode::TranscodeStep, Step};
use std::collections::HashMap;
use std::sync::Arc;

pub fn register_all(map: &mut HashMap<String, Arc<dyn Step>>) {
    map.insert("probe".into(), Arc::new(ProbeStep));
    map.insert("transcode".into(), Arc::new(TranscodeStep));
    map.insert("output".into(), Arc::new(OutputStep));
    // verify.playable, remux, …, registered when their files are added (Tasks 8-9)
}
