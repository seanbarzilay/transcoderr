pub mod discord;
pub mod ntfy;
pub mod telegram;
pub mod webhook;

use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn send(&self, message: &str, extra: &Value) -> anyhow::Result<()>;
}

pub fn build(kind: &str, config: &Value) -> anyhow::Result<Box<dyn Notifier>> {
    match kind {
        "discord"  => Ok(Box::new(discord::Discord::new(config)?)),
        "ntfy"     => Ok(Box::new(ntfy::Ntfy::new(config)?)),
        "telegram" => Ok(Box::new(telegram::Telegram::new(config)?)),
        "webhook"  => Ok(Box::new(webhook::WebhookNotifier::new(config)?)),
        other     => anyhow::bail!("unknown notifier kind {other}"),
    }
}
