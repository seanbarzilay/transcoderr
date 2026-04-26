use clap::ValueEnum;

/// Log output format selectable at startup.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum LogFormat {
    Text,
    Json,
}

/// Initialize the global tracing subscriber.
///
/// `default_filter` is used when `RUST_LOG` is unset
/// (e.g. `"transcoderr=info,tower_http=info"`).
///
/// Installs a process-wide singleton subscriber and panics if called more than once.
pub fn init(format: LogFormat, default_filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);

    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_target(true)
            .init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_json_does_not_panic() {
        init(LogFormat::Json, "info");
        tracing::info!(test = "smoke", "hello from logging init");
    }
}
