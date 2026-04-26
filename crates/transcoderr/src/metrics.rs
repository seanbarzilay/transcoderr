use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub struct Metrics {
    pub handle: PrometheusHandle,
}

impl Metrics {
    pub fn install() -> anyhow::Result<Self> {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .map_err(|e| anyhow::anyhow!("install metrics recorder: {e}"))?;
        Ok(Self { handle })
    }
    pub fn render(&self) -> String { self.handle.render() }
}

pub fn record_job_finished(flow: &str, status: &str, duration_secs: f64) {
    metrics::counter!("transcoderr_jobs_total", "flow" => flow.to_string(), "status" => status.to_string()).increment(1);
    metrics::histogram!("transcoderr_job_duration_seconds", "flow" => flow.to_string(), "status" => status.to_string()).record(duration_secs);
}

pub fn record_step_finished(plugin: &str, status: &str, duration_secs: f64) {
    metrics::histogram!("transcoderr_step_duration_seconds", "plugin" => plugin.to_string(), "status" => status.to_string()).record(duration_secs);
}

pub fn set_queue_depth(depth: i64) {
    metrics::gauge!("transcoderr_queue_depth").set(depth as f64);
}
