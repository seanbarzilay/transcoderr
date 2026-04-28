//! One-shot reconciler that runs at boot. For each auto-provisioned
//! source (kind in radarr/sonarr/lidarr AND `arr_notification_id`
//! present in `config_json`), fetch the corresponding notification
//! from the *arr and verify URL + secret_token still match. If
//! either drifted, DELETE + recreate. Cosmetic drift (event flags,
//! display name) is intentionally tolerated.

use crate::arr;
use crate::db;
use sqlx::SqlitePool;
use std::sync::Arc;

pub fn spawn(pool: SqlitePool, public_url: Arc<String>) {
    tokio::spawn(async move {
        if let Err(e) = run(&pool, &public_url).await {
            tracing::warn!(error = %e, "boot reconciler failed; sources may be in an unexpected state");
        }
    });
}

async fn run(pool: &SqlitePool, public_url: &str) -> anyhow::Result<()> {
    let sources = db::sources::list_all(pool).await?;
    for src in sources {
        let Some(arr_kind) = arr::Kind::parse(&src.kind) else { continue };
        let cfg: serde_json::Value = match serde_json::from_str(&src.config_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(source_id = src.id, error = %e, "skipping source during reconcile: invalid config_json");
                continue;
            }
        };
        let Some(notification_id) = cfg.get("arr_notification_id").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some(base_url) = cfg.get("base_url").and_then(|v| v.as_str()) else { continue };
        let Some(api_key) = cfg.get("api_key").and_then(|v| v.as_str()) else { continue };

        if let Err(e) = reconcile_one(pool, &src, arr_kind, base_url, api_key, notification_id, public_url).await {
            tracing::warn!(source_id = src.id, name = %src.name, error = ?e, "reconcile failed");
        }
    }
    Ok(())
}

async fn reconcile_one(
    pool: &SqlitePool,
    src: &db::sources::SourceRow,
    arr_kind: arr::Kind,
    base_url: &str,
    api_key: &str,
    notification_id: i64,
    public_url: &str,
) -> anyhow::Result<()> {
    let client = arr::Client::new(base_url, api_key)?;
    let expected_url = format!("{public_url}/webhook/{}", src.kind);

    match client.get_notification(notification_id).await? {
        Some(n) if matches_expected(&n, &expected_url, &src.secret_token) => {
            tracing::info!(source_id = src.id, notification_id, "*arr webhook in sync");
        }
        Some(_) => {
            tracing::warn!(
                source_id = src.id,
                notification_id,
                expected_url = %expected_url,
                "*arr webhook drifted on key fields; recreating"
            );
            client.delete_notification(notification_id).await?;
            let new_n = client
                .create_notification(arr_kind, &src.name, &expected_url, &src.secret_token)
                .await?;
            db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
            tracing::info!(source_id = src.id, old_id = notification_id, new_id = new_n.id, "*arr webhook recreated");
        }
        None => {
            // The tracked id is gone. Before we create from scratch, check
            // whether a notification with our expected name already exists
            // on the *arr — operator manual recreate, or a prior partial
            // reconcile cycle, can leave one behind. Without this lookup,
            // the create-from-scratch hits a "Should be unique" 400 from
            // the *arr and the source stays out of sync forever.
            let expected_name = format!("transcoderr-{}", src.name);
            let existing = client
                .list_notifications()
                .await?
                .into_iter()
                .find(|n| n.name == expected_name);
            if let Some(found) = existing {
                let found_id = found.id;
                if matches_expected(&found, &expected_url, &src.secret_token) {
                    db::sources::update_arr_notification_id(pool, src.id, found_id).await?;
                    tracing::info!(source_id = src.id, adopted_id = found_id, "*arr webhook adopted by name");
                } else {
                    tracing::warn!(source_id = src.id, adopted_id = found_id,
                        "adopted *arr webhook is drifted on key fields; recreating");
                    client.delete_notification(found_id).await?;
                    let new_n = client
                        .create_notification(arr_kind, &src.name, &expected_url, &src.secret_token)
                        .await?;
                    db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
                    tracing::info!(source_id = src.id, old_id = found_id, new_id = new_n.id, "*arr webhook recreated");
                }
            } else {
                tracing::warn!(source_id = src.id, missing_id = notification_id, "*arr webhook missing; recreating");
                let new_n = client
                    .create_notification(arr_kind, &src.name, &expected_url, &src.secret_token)
                    .await?;
                db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
                tracing::info!(source_id = src.id, new_id = new_n.id, "*arr webhook recreated");
            }
        }
    }
    Ok(())
}

/// Drift detection — only the fields that break delivery. Cosmetic
/// drift (operator-toggled event flags, renamed display name, added
/// tags) is intentionally ignored.
pub(crate) fn matches_expected(
    n: &arr::Notification,
    expected_url: &str,
    expected_secret: &str,
) -> bool {
    let url = n
        .fields
        .iter()
        .find(|f| f.name == "url")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");
    let password = n
        .fields
        .iter()
        .find(|f| f.name == "password")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");
    url == expected_url && password == expected_secret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::{Field, Notification};

    fn n(url: &str, password: &str) -> Notification {
        Notification {
            id: 1,
            name: "transcoderr-x".into(),
            implementation: "Webhook".into(),
            config_contract: "WebhookSettings".into(),
            fields: vec![
                Field { name: "url".into(), value: serde_json::json!(url) },
                Field { name: "password".into(), value: serde_json::json!(password) },
            ],
            on_grab: false,
            on_download: true,
            on_upgrade: true,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn matches_expected_when_url_and_secret_match() {
        let notif = n("http://t/webhook/radarr", "abc");
        assert!(matches_expected(&notif, "http://t/webhook/radarr", "abc"));
    }

    #[test]
    fn does_not_match_when_url_drifted() {
        let notif = n("http://OLD/webhook/radarr", "abc");
        assert!(!matches_expected(&notif, "http://NEW/webhook/radarr", "abc"));
    }

    #[test]
    fn does_not_match_when_secret_drifted() {
        let notif = n("http://t/webhook/radarr", "OLD");
        assert!(!matches_expected(&notif, "http://t/webhook/radarr", "NEW"));
    }

    #[test]
    fn matches_when_only_event_flags_drifted() {
        let mut notif = n("http://t/webhook/radarr", "abc");
        notif.on_grab = true;
        notif.name = "operator-renamed".into();
        notif.extra.insert("tags".into(), serde_json::json!([1, 2]));
        assert!(matches_expected(&notif, "http://t/webhook/radarr", "abc"));
    }
}
