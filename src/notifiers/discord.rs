use super::Notifier;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct Discord { url: String }

impl Discord {
    pub fn new(cfg: &Value) -> anyhow::Result<Self> {
        let url = cfg["url"].as_str().ok_or_else(|| anyhow::anyhow!("discord: missing url"))?.to_string();
        Ok(Self { url })
    }
}

#[async_trait]
impl Notifier for Discord {
    async fn send(&self, message: &str, _extra: &Value) -> anyhow::Result<()> {
        let body = json!({ "content": message });
        let resp = reqwest::Client::new().post(&self.url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("discord {}: {}", resp.status(), resp.text().await.unwrap_or_default());
        }
        Ok(())
    }
}
