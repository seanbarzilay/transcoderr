use super::Notifier;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct WebhookNotifier { url: String }

impl WebhookNotifier {
    pub fn new(cfg: &Value) -> anyhow::Result<Self> {
        let url = cfg["url"].as_str().ok_or_else(|| anyhow::anyhow!("webhook: missing url"))?.to_string();
        Ok(Self { url })
    }
}

#[async_trait]
impl Notifier for WebhookNotifier {
    async fn send(&self, message: &str, extra: &Value) -> anyhow::Result<()> {
        let body = json!({ "message": message, "extra": extra });
        let resp = reqwest::Client::new().post(&self.url).json(&body).send().await?;
        if !resp.status().is_success() { anyhow::bail!("webhook: {}", resp.status()); }
        Ok(())
    }
}
