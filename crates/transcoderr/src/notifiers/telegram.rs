use super::Notifier;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct Telegram {
    base_url: String,
    bot_token: String,
    chat_id: String,
}

impl Telegram {
    pub fn new(cfg: &Value) -> anyhow::Result<Self> {
        let bot_token = cfg["bot_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("telegram: missing bot_token"))?
            .to_string();
        let chat_id = cfg["chat_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("telegram: missing chat_id"))?
            .to_string();
        let base_url = cfg["base_url"]
            .as_str()
            .unwrap_or("https://api.telegram.org")
            .trim_end_matches('/')
            .to_string();
        Ok(Self { base_url, bot_token, chat_id })
    }
}

#[async_trait]
impl Notifier for Telegram {
    async fn send(&self, message: &str, _extra: &Value) -> anyhow::Result<()> {
        let url = format!("{}/bot{}/sendMessage", self.base_url, self.bot_token);
        let body = json!({ "chat_id": self.chat_id, "text": message });
        let resp = reqwest::Client::new().post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "telegram {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }
}
