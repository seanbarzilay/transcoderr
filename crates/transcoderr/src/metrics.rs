use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub struct Metrics {
    pub handle: PrometheusHandle,
}

impl Metrics {
    pub fn install() -> anyhow::Result<Self> {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .map_err(|e| anyhow::anyhow!("install metrics recorder: {e}"))?;
        // Register descriptors up front so `/metrics` always has
        // # HELP / # TYPE lines, even before the worker has touched a
        // counter/gauge. Without this, the endpoint returns an empty
        // body until something records a value, which races with any
        // process that scrapes /metrics at boot.
        metrics::describe_counter!(
            "transcoderr_jobs_total",
            "Total transcode jobs by flow + terminal status"
        );
        metrics::describe_histogram!(
            "transcoderr_job_duration_seconds",
            "Wall-clock duration of completed jobs by flow + status"
        );
        metrics::describe_histogram!(
            "transcoderr_step_duration_seconds",
            "Wall-clock duration of plan steps by plugin + status"
        );
        metrics::describe_gauge!(
            "transcoderr_queue_depth",
            "Current pending+running job count"
        );
        // Seed the gauge so the metric appears with a value from boot.
        metrics::gauge!("transcoderr_queue_depth").set(0.0);
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
