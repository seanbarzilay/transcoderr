# Source Auto-Provisioning — Design

**Date:** 2026-04-27
**Branch:** `feature/source-autoprovision`
**Status:** Draft, pending implementation
**Builds on:** v0.9.0 (commit `d2affae`)

## Goal

Replace the "operator manually configures the webhook in Radarr/Sonarr"
UX with a model where transcoderr auto-provisions the webhook on the
*arr's side when the operator creates a source, deletes it on the *arr
when the source is removed, reconciles drift on update, and validates
existence at boot. The operator only ever inputs the *arr's base URL +
API key; transcoderr handles everything else.

The pain point: today's flow requires the operator to copy the
webhook URL into the *arr's Settings → Connect → Webhook page, paste
the secret token they got back from `create_source`, and tick the
right event-type checkboxes. Three steps, one of them error-prone
(token can be mis-pasted), and there's no consistency check — if the
operator later edits or deletes the webhook in the *arr, transcoderr
won't notice.

## Scope

- `radarr` / `sonarr` / `lidarr` source kinds get auto-provisioning.
  All three are servarr forks and share the same `/api/v3/notification`
  endpoint shape.
- `generic` and `webhook` source kinds keep today's manual flow
  unchanged. There's no API to call for arbitrary webhooks.

## Non-goals

- **Periodic background reconciliation.** Boot is one-shot. If the
  operator manually edits the webhook in the *arr's UI between
  transcoderr restarts, transcoderr won't catch it until the next
  boot. Adding a `tokio::time::interval` is a small follow-up if
  anyone reports the drift gap as a real problem.
- **Encryption at rest for `api_key` and `secret_token`.** Both live
  as plain strings in SQLite, protected by filesystem permissions
  (same as today's `secret_token`). Real encryption needs a
  master-key story.
- **Webhook-test endpoint.** Exposing `POST /api/sources/{id}/test`
  that calls the *arr's `/notification/test` is useful for validation
  but not load-bearing.
- **Loading `api_key` from a docker secret / file.** Same trust model
  as today's other secrets.
- **Migration of legacy v0.9.x sources.** Legacy sources (no
  `arr_notification_id` in their config) keep working untouched. The
  reconciler skips them; the manual webhook setup the operator did at
  install time still routes inbound events. Operators who want auto-
  provisioning re-create the source through the new flow.

## Design

### 1. New `arr` module — typed client

New file `crates/transcoderr/src/arr/mod.rs`. Thin typed wrapper
around the three *arr `/api/v3/notification` operations.

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Radarr, Sonarr, Lidarr }

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "radarr" => Some(Kind::Radarr),
            "sonarr" => Some(Kind::Sonarr),
            "lidarr" => Some(Kind::Lidarr),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub name: String,
    pub implementation: String,
    pub config_contract: String,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub on_grab: bool,
    #[serde(default)]
    pub on_download: bool,
    #[serde(default)]
    pub on_upgrade: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: serde_json::Value,
}

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl Client {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self>;
    pub async fn list_notifications(&self) -> Result<Vec<Notification>>;
    pub async fn get_notification(&self, id: i64) -> Result<Option<Notification>>;
    pub async fn create_notification(&self, kind: Kind, name: &str, webhook_url: &str, secret: &str) -> Result<Notification>;
    pub async fn delete_notification(&self, id: i64) -> Result<()>;
}
```

The webhook payload built by `create_notification` matches the *arr's
Webhook implementation:

```json
{
  "name": "transcoderr-{name}",
  "implementation": "Webhook",
  "configContract": "WebhookSettings",
  "fields": [
    { "name": "url", "value": "{webhook_url}" },
    { "name": "method", "value": 1 },
    { "name": "username", "value": "" },
    { "name": "password", "value": "{secret}" }
  ],
  "onGrab": false,
  "onDownload": true,
  "onUpgrade": true
}
```

The `password` field carries the secret token because the existing
webhook handlers already expect Basic auth with the secret in the
password slot — same as today's manual setup.

Per-kind event-flag presets live in a `event_flags(kind: Kind)` helper:
sonarr also enables `onSeriesAdd` and `onEpisodeFileDelete`; lidarr's
event names follow its own naming. The helper keeps the
kind-specific knowledge in one place.

API key goes in the `X-Api-Key` header. HTTP timeout is 15 seconds
per request — generous for typical homelab latencies, tight enough
that an unreachable *arr fails fast.

### 2. Public URL resolution

New file `crates/transcoderr/src/public_url.rs`:

```rust
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy)]
pub enum Source { Env, Default }

#[derive(Debug, Clone)]
pub struct PublicUrl {
    pub url: String,
    pub source: Source,
}

/// Resolve from `TRANSCODERR_PUBLIC_URL` if set, else
/// `http://{gethostname()}:{addr.port()}`. Falls back to `localhost`
/// if the gethostname() syscall fails.
pub fn resolve(bound_addr: SocketAddr) -> PublicUrl {
    if let Ok(url) = std::env::var("TRANSCODERR_PUBLIC_URL") {
        let url = url.trim_end_matches('/').to_string();
        return PublicUrl { url, source: Source::Env };
    }
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    let url = format!("http://{host}:{}", bound_addr.port());
    PublicUrl { url, source: Source::Default }
}
```

Adds the `hostname` crate (`hostname = "0.4"`). Resolved at boot in
`main.rs::serve` after the listener is bound, logged at info, and
stored in `AppState::public_url: Arc<String>`. The source-create
handler reads it to build the URL passed to the *arr.

For docker-compose deployments the default works out of the box: the
container hostname is the service name, so `http://transcoderr:8099`
is what sibling containers (radarr, sonarr) reach. Custom deployments
with a reverse proxy or different hostname override via the env var.

**Webhook URL shape**: the URL passed to the *arr is
`{public_url}/api/webhooks/{kind}` (no token in the URL — it's
delivered via Basic auth in the password slot).

### 3. Source-create auto-provision flow

`crates/transcoderr/src/api/sources.rs::create` is reworked. When
`kind` is auto-provision-eligible:

1. Validate `req.config.base_url` and `req.config.api_key` are present.
2. Generate a fresh `secret_token` (random 32-byte hex).
3. Construct `webhook_url = format!("{}/api/webhooks/{}", state.public_url, kind)`.
4. Build an `arr::Client` from base_url + api_key. Call
   `create_notification(arr_kind, name, webhook_url, secret_token)`.
5. On success, persist the source row with `config_json` extended to
   include `arr_notification_id: <returned-id>`.
6. Return the redacted SourceSummary.

Atomicity: validation runs before any DB write, the *arr call runs
before any DB write. If the *arr returns 4xx, no local row is created
and the error message surfaces to the operator (e.g. "Radarr returned
401 Unauthorized — check your API key").

`generic` / `webhook` kinds bypass all of this and use today's flow:
the operator passes `secret_token` and `config` directly; transcoderr
just stores them.

**Config shape** for auto-provisioned sources after create:

```json
{
  "base_url": "http://radarr:7878",
  "api_key": "...",
  "arr_notification_id": 42
}
```

**Redaction**: `api_key` joins `secret_token` in the redacted set.
Token-authed callers see `api_key: "***"`; UI session callers see
plain. Same trust model as today's `secret_token`.

### 4. Source-delete symmetric teardown

`api/sources.rs::delete` reads the row first, calls
`Client::delete_notification(arr_notification_id)` if the source is
auto-provisioned, then deletes the local row regardless of the *arr
response.

```rust
let row = db::sources::get_by_id(&pool, id).await?
    .ok_or_else(|| api_err("source.not_found", ...))?;

let cfg: Value = serde_json::from_str(&row.config_json).unwrap_or_default();
let kind_parsed = arr::Kind::parse(&row.kind);
let notification_id = cfg.get("arr_notification_id").and_then(|v| v.as_i64());

if let (Some(arr_kind), Some(notification_id)) = (kind_parsed, notification_id) {
    if let (Some(base), Some(key)) = (
        cfg.get("base_url").and_then(|v| v.as_str()),
        cfg.get("api_key").and_then(|v| v.as_str()),
    ) {
        match arr::Client::new(base, key) {
            Ok(c) => match c.delete_notification(notification_id).await {
                Ok(()) => tracing::info!(source_id = id, notification_id, "deleted *arr webhook"),
                Err(e) => tracing::warn!(source_id = id, notification_id, error = %e,
                    "failed to delete *arr webhook; proceeding with local delete"),
            },
            Err(e) => tracing::warn!(source_id = id, error = %e,
                "failed to construct arr client; proceeding with local delete"),
        }
    }
}

db::sources::delete(&pool, id).await?;
```

Behavior:
- Auto-provisioned source → try the *arr DELETE; success or failure logs; always delete locally.
- Legacy sources (no `arr_notification_id`) → skip remote teardown; local-only delete.
- Unreachable *arr → warn and complete the local delete. The dangling notification on the *arr's side is the operator's problem; they explicitly asked for the source to be gone.

### 5. Source-update auto-reconcile

`api/sources.rs::update` gets the same treatment. After computing the
new config (merging req fields with the existing row), decide whether
the *arr-side webhook needs to be recreated:

```rust
let needs_reprovision = arr_kind.is_some()
    && old_cfg.get("arr_notification_id").is_some()
    && (
        old_cfg.get("base_url") != new_cfg.get("base_url") ||
        old_cfg.get("api_key") != new_cfg.get("api_key") ||
        new_name != row.name
    );
```

If yes:
1. **Best-effort delete** of the OLD webhook against the OLD
   base_url/api_key. Log + proceed if unreachable (the old *arr might
   be gone if the operator is migrating).
2. **Mandatory create** of the NEW webhook against the NEW
   base_url/api_key. If this fails, the entire update fails (no
   half-applied state in the DB).
3. On success, write the new config (with the new `arr_notification_id`)
   to the local row.

If no (only cosmetic fields changed, e.g. name on a `generic` source):
just update the DB row. No *arr calls.

**`secret_token` is unchanged by update** — once provisioned, the
token stays stable across updates so already-cached deliveries from
the *arr don't 401. To rotate the token, the operator deletes and
re-creates the source.

### 6. Boot reconciler

After the HTTP listener is bound and `axum::serve` is spawned, fire
off a one-shot reconciler task. Best-effort, log-and-continue.

New file `crates/transcoderr/src/arr/reconcile.rs`:

```rust
pub fn spawn(pool: SqlitePool, public_url: Arc<String>) {
    tokio::spawn(async move {
        if let Err(e) = run(&pool, &public_url).await {
            tracing::warn!(error = %e, "boot reconciler failed; sources may be in an unexpected state");
        }
    });
}

async fn run(pool: &SqlitePool, public_url: &str) -> anyhow::Result<()> {
    let sources = db::sources::list(pool).await?;
    for src in sources {
        let Some(arr_kind) = arr::Kind::parse(&src.kind) else { continue };
        let cfg: Value = serde_json::from_str(&src.config_json).unwrap_or_default();
        let Some(notification_id) = cfg.get("arr_notification_id").and_then(|v| v.as_i64()) else { continue };
        let Some(base_url) = cfg.get("base_url").and_then(|v| v.as_str()) else { continue };
        let Some(api_key) = cfg.get("api_key").and_then(|v| v.as_str()) else { continue };

        if let Err(e) = reconcile_one(pool, &src, arr_kind, base_url, api_key, notification_id, public_url).await {
            tracing::warn!(source_id = src.id, name = %src.name, error = %e, "reconcile failed");
        }
    }
    Ok(())
}

async fn reconcile_one(
    pool: &SqlitePool,
    src: &SourceRow,
    arr_kind: arr::Kind,
    base_url: &str,
    api_key: &str,
    notification_id: i64,
    public_url: &str,
) -> anyhow::Result<()> {
    let client = arr::Client::new(base_url, api_key)?;
    let expected_url = format!("{public_url}/api/webhooks/{}", src.kind);
    match client.get_notification(notification_id).await? {
        Some(n) if matches_expected(&n, &expected_url, &src.secret_token) => {
            tracing::info!(source_id = src.id, notification_id, "*arr webhook in sync");
        }
        Some(_) => {
            // Drift on URL or secret. Recreate.
            tracing::warn!(source_id = src.id, notification_id, "*arr webhook drifted; recreating");
            client.delete_notification(notification_id).await?;
            let new_n = client.create_notification(arr_kind, &src.name, &expected_url, &src.secret_token).await?;
            db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
        }
        None => {
            // Missing entirely. Recreate.
            tracing::warn!(source_id = src.id, missing_id = notification_id, "*arr webhook missing; recreating");
            let new_n = client.create_notification(arr_kind, &src.name, &expected_url, &src.secret_token).await?;
            db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
        }
    }
    Ok(())
}

fn matches_expected(n: &arr::Notification, expected_url: &str, expected_secret: &str) -> bool {
    let url = n.fields.iter().find(|f| f.name == "url").and_then(|f| f.value.as_str()).unwrap_or("");
    let password = n.fields.iter().find(|f| f.name == "password").and_then(|f| f.value.as_str()).unwrap_or("");
    url == expected_url && password == expected_secret
}
```

**Drift detection** is convergent on the fields that matter for
delivery (URL + password) and tolerant of cosmetic fields:
- Operator added an extra event flag in the *arr's UI → preserved.
- Operator renamed the webhook → preserved (we match by stored
  `arr_notification_id`, not by name).
- URL changed (e.g. transcoderr's hostname changed) → recreated to
  the new expected URL.
- Password changed (operator manually rotated) → recreated to match
  what transcoderr expects.

Wired in `main.rs::serve` after `axum::serve`:

```rust
transcoderr::arr::reconcile::spawn(state.pool.clone(), state.public_url.clone());
```

### 7. WebUI source-create form

`web/` (the React frontend) gains conditional rendering on the source
create / edit form.

For **radarr / sonarr / lidarr**:
- `Name` (text, required)
- `Base URL` (text, required, placeholder `http://radarr:7878`)
- `API key` (password input, required)
- Help text: "*Transcoderr will create the webhook in
  {Radarr|Sonarr|Lidarr} for you. The connection token is generated
  automatically.*"
- **No secret_token field** — server generates it.

For **generic** (today's flow):
- `Name`, `Secret token`, `Config` — unchanged.
- Help text: "*Add a webhook in your tool's settings pointing at
  `{public_url}/api/webhooks/generic` with this token as the
  password.*" — surfaces the URL the operator needs to paste.

**Edit form** for an auto-provisioned source: `Base URL` and `API
key` editable; `API key` field shows `***` placeholder + a "Replace"
button to enter a new key. Keeping the placeholder means "unchanged"
(matches the existing API's `"***"` convention).

**On submit error**: surface the *arr's HTTP error message inline
near the field (e.g. "Radarr returned 401 — check your API key" near
the API key field) rather than a generic toast.

**List view**: show a small "auto" / "manual" badge per source so
operators can tell which kind of source they're looking at.

Files (rough estimate, confirmed during implementation):
- `web/src/components/SourceForm.tsx` — kind-conditional fields
- `web/src/api/sources.ts` — types updated for the new fields
- `web/src/pages/SourcesPage.tsx` — list view auto/manual badge

## Testing

**Pure-function / unit tests:**

1. `arr::Kind::parse` — radarr/sonarr/lidarr → Some, generic/garbage → None.
2. `matches_expected` — URL+secret match → true; URL drift → false;
   secret drift → false; cosmetic drift (extra event flag) → true.
3. `public_url::resolve` — env var set → `Source::Env`; env unset → 
   `"http://{hostname}:{port}"` and `Source::Default`. The
   hostname-fails fallback is exercised by reading `gethostname()` in
   the test (not stubbed; the test asserts the URL contains the actual
   bound port and the host segment is non-empty).

**Mocked-HTTP tests** for `arr::Client` (using `wiremock = "0.6"` as a
dev-dep):

4. `create_notification_builds_correct_payload` — assert the captured
   request body has `implementation: "Webhook"`, the URL field, and
   `password: <secret>`.
5. `delete_notification_passes_id_in_path` — assert the mock saw
   `DELETE /api/v3/notification/42` with `X-Api-Key` header.
6. `get_notification_returns_none_on_404` — mock returns 404; client
   returns `Ok(None)`.
7. `create_notification_surfaces_arr_error_message` — mock returns 401
   with `{"message": "Unauthorized"}`; client error chain includes
   `Unauthorized`.

**Integration test** for the create-source flow:

8. `create_source_radarr_calls_arr_then_persists` — wiremock plays the
   role of Radarr; transcoderr's `POST /api/sources` lands; mock saw
   POST to `/api/v3/notification`; local DB has a row with
   `arr_notification_id` set; response body redacts `api_key`.

**Skipped:**

- Boot reconciler integration test — requires both transcoderr's full
  app start AND a mock *arr running concurrently; too much harness
  for the value. The pure-function `matches_expected` test plus the
  create-source integration test cover the load-bearing logic.
- WebUI form tests — the form is React; the API/server tests cover
  the wire contract. WebUI changes are visually verifiable.

**Manual end-to-end:**

- Deploy the binary; in Radarr, capture an API key from
  Settings → General → API Key.
- Through the WebUI: create a source with kind=radarr, base URL of
  the local Radarr, the API key. Confirm Radarr's
  Settings → Connect now shows a "transcoderr-{name}" webhook.
- Trigger an event in Radarr (e.g. mark a movie as upgraded).
  Confirm transcoderr saw the webhook and started a flow run.
- In Radarr, manually delete the webhook. Restart transcoderr.
  Confirm logs show `*arr webhook missing; recreating` and the
  webhook is back.
- Update the source's API key in transcoderr (e.g. to a wrong value).
  Confirm the API call returns the *arr's 401 error message
  inline.
- Delete the source. Confirm Radarr's Settings → Connect no longer
  shows the webhook.

## Acceptance

The branch is ready to merge when:

- New `arr` module with the typed Client + Kind + Notification types
  exists.
- `public_url::resolve` runs at boot in `main.rs::serve`; result
  stored in `AppState::public_url`.
- `POST /api/sources` with kind=radarr/sonarr/lidarr calls the *arr's
  `POST /api/v3/notification` and stores the returned ID in
  `config_json.arr_notification_id`.
- `DELETE /api/sources/{id}` calls the *arr's
  `DELETE /api/v3/notification/{id}` for auto-provisioned sources.
- `PUT /api/sources/{id}` reprovisions the *arr-side webhook when
  base_url, api_key, or name change on an auto-provisioned source.
- Boot reconciler spawned in `serve`; verifies each auto-provisioned
  source against the *arr; recreates on drift.
- `api_key` is redacted to `***` in token-authed responses.
- WebUI source form renders kind-conditional fields and submits the
  new `base_url` / `api_key` payload for auto-provision kinds.
- The 8 tests above pass; `cargo test -p transcoderr --locked --lib --tests`
  passes (the pre-existing metrics flake notwithstanding).
- Manual end-to-end against a real Radarr instance confirms the
  full lifecycle (create → boot reconcile → update → delete).
