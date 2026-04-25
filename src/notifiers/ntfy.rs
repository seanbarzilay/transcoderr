use super::Notifier;
use async_trait::async_trait;
use serde_json::Value;

pub struct Ntfy { server: String, topic: String }

impl Ntfy {
    pub fn new(cfg: &Value) -> anyhow::Result<Self> {
        Ok(Self {
            server: cfg["server"].as_str().unwrap_or("https://ntfy.sh").to_string(),
            topic:  cfg["topic"].as_str().ok_or_else(|| anyhow::anyhow!("ntfy: missing topic"))?.to_string(),
        })
    }
}

#[async_trait]
impl Notifier for Ntfy {
    async fn send(&self, message: &str, _extra: &Value) -> anyhow::Result<()> {
        let url = format!("{}/{}", self.server.trim_end_matches('/'), self.topic);
        let resp = reqwest::Client::new().post(&url).body(message.to_string()).send().await?;
        if !resp.status().is_success() { anyhow::bail!("ntfy: {}", resp.status()); }
        Ok(())
    }
}
