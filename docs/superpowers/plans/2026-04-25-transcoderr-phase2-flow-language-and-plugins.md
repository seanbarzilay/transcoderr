# transcoderr Phase 2 — Full Flow Language + Plugin Host Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the flow language up to spec — conditionals, expressions, `return:`, `on_failure`, `retry:`, `match.expr` — and add a subprocess-based plugin host so external plugins can ship new step types. Add Sonarr, Lidarr, and generic webhook adapters. Replace the single-token Radarr config with a real `sources` table.

**Architecture:** Reuses the Phase 1 engine and worker. The flow engine grows a recursive `execute_node` that handles conditional/return/step nodes. Expressions go through CEL (`cel-interpreter` crate). The plugin host implements the `Step` trait for subprocess plugins, spawning a child process and exchanging newline-delimited JSON-RPC messages over stdio. Built-ins keep their in-process implementation but expose the same manifest+schema metadata as external plugins.

**Tech Stack:** Rust, `cel-interpreter`, `tokio::process`, `serde_json`, `jsonschema` (validation), all carried over from Phase 1.

---

## Scope

**In:**
- Conditional nodes (`if/then/else`), nested
- `return: <label>` terminal nodes
- `match.expr` — flow-level filter on trigger payload
- `on_failure:` flow-level handler
- `retry:` per-step
- CEL expression evaluator wired into all four sites
- `sources` table + Sources CRUD
- Webhook adapters: Sonarr, Lidarr, generic `POST /webhook/:name`
- Webhook dedup window (configurable, default 5 min)
- Plugin host: subprocess JSON-RPC, plugin discovery from `data/plugins/`
- Manifest + schema validation (built-ins ship synthetic manifests)
- Built-in steps added: `verify.playable`, `remux`, `extract.subs`, `strip.tracks`, `move`, `copy`, `delete`, `notify`, `shell`
- First-party notifiers: Discord, ntfy, generic webhook
- Per-step timeout enforcement

**Out:**
- GPU / hardware capabilities → Phase 3
- Web UI / JSON API → Phase 4
- Prometheus / retention / log spillover / auth / Docker → Phase 5

---

## File Structure (delta from Phase 1)

```
migrations/
  20260425000002_phase2_sources.sql              (sources, plugins, notifiers tables)
src/
  flow/
    expr.rs                                       CEL evaluator wrapper + context binding
    model.rs                                      EXTENDED: add Conditional, Return, OnFailure, Retry
    parser.rs                                     EXTENDED: parse new node shapes
    engine.rs                                     EXTENDED: recursive execute_node, on_failure, retry
  steps/
    mod.rs                                        EXTENDED: registry combines built-ins + discovered plugins
    verify_playable.rs
    remux.rs
    extract_subs.rs
    strip_tracks.rs
    move_step.rs
    copy_step.rs
    delete_step.rs
    notify.rs
    shell.rs
  plugins/
    mod.rs                                        Plugin discovery + manifest loading
    manifest.rs                                   manifest.toml parser
    subprocess.rs                                 Subprocess Step implementation (JSON-RPC over stdio)
  notifiers/
    mod.rs                                        Notifier trait + dispatch
    discord.rs
    ntfy.rs
    webhook.rs
  http/
    webhook_sonarr.rs
    webhook_lidarr.rs
    webhook_generic.rs
    dedup.rs                                      In-memory LRU keyed by (source, file_path, payload_hash)
  db/
    sources.rs
    plugins.rs                                    DB-side plugin registry (mirrors discovered manifests)
    notifiers.rs
tests/
  flow_conditional.rs
  flow_return.rs
  flow_match_expr.rs
  flow_on_failure.rs
  flow_retry.rs
  webhook_sonarr.rs
  webhook_generic.rs
  plugin_subprocess.rs
  notify_discord.rs                              (uses a mock HTTP server)
  step_verify_playable.rs
  step_remux.rs
  webhook_dedup.rs
  fixtures/
    plugins/
      hello/
        manifest.toml
        schema.json
        bin/run                                  bash test plugin
```

---

## Tasks

### Task 1: Migration — sources, plugins, notifiers tables

**Files:**
- Create: `migrations/20260425000002_phase2_sources.sql`

- [ ] **Step 1: Write the migration**

```sql
CREATE TABLE sources (
  id            INTEGER PRIMARY KEY,
  kind          TEXT NOT NULL,           -- 'radarr'|'sonarr'|'lidarr'|'webhook'
  name          TEXT NOT NULL UNIQUE,
  config_json   TEXT NOT NULL,
  secret_token  TEXT NOT NULL
);

CREATE TABLE plugins (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  version       TEXT NOT NULL,
  kind          TEXT NOT NULL,           -- 'builtin'|'subprocess'
  path          TEXT,
  schema_json   TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE notifiers (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  kind          TEXT NOT NULL,           -- 'discord'|'ntfy'|'webhook'
  config_json   TEXT NOT NULL
);

-- jobs table: add source_id FK (nullable for backfill, NOT NULL going forward)
ALTER TABLE jobs ADD COLUMN source_id INTEGER REFERENCES sources(id);
CREATE INDEX idx_jobs_dedup ON jobs(source_id, file_path, created_at);
```

- [ ] **Step 2: Verify migration applies**

Run: `cargo test db::tests::opens_and_migrates`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add migrations/20260425000002_phase2_sources.sql
git commit -m "feat(db): sources, plugins, notifiers tables"
```

---

### Task 2: Sources CRUD

**Files:**
- Create: `src/db/sources.rs`
- Modify: `src/db/mod.rs`
- Create: `tests/db_sources.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/db_sources.rs`:

```rust
use serde_json::json;
use tempfile::tempdir;
use transcoderr::db;

#[tokio::test]
async fn source_crud_roundtrip() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let id = db::sources::insert(&pool, "radarr", "main", &json!({"url":"http://radarr"}), "tok").await.unwrap();
    let s = db::sources::get_by_kind_and_token(&pool, "radarr", "tok").await.unwrap().unwrap();
    assert_eq!(s.id, id);
    assert_eq!(s.name, "main");
}
```

- [ ] **Step 2: Implement**

Create `src/db/sources.rs`:

```rust
use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SourceRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub config_json: String,
    pub secret_token: String,
}

pub async fn insert(pool: &SqlitePool, kind: &str, name: &str, config: &Value, token: &str) -> anyhow::Result<i64> {
    let cj = serde_json::to_string(config)?;
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO sources (kind, name, config_json, secret_token) VALUES (?, ?, ?, ?) RETURNING id"
    ).bind(kind).bind(name).bind(cj).bind(token).fetch_one(pool).await?)
}

pub async fn get_by_kind_and_token(pool: &SqlitePool, kind: &str, token: &str) -> anyhow::Result<Option<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources WHERE kind = ? AND secret_token = ?")
        .bind(kind).bind(token).fetch_optional(pool).await?)
}

pub async fn get_webhook_by_name_and_token(pool: &SqlitePool, name: &str, token: &str) -> anyhow::Result<Option<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources WHERE kind = 'webhook' AND name = ? AND secret_token = ?")
        .bind(name).bind(token).fetch_optional(pool).await?)
}
```

Add to `src/db/mod.rs`:

```rust
pub mod sources;
```

- [ ] **Step 3: Run**

Run: `cargo test --test db_sources`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/db/sources.rs src/db/mod.rs tests/db_sources.rs
git commit -m "feat(db): sources CRUD"
```

---

### Task 3: Extend flow AST + parser for conditionals, return, retry, on_failure, match.expr

**Files:**
- Modify: `src/flow/model.rs`
- Modify: `src/flow/parser.rs`
- Create: `tests/flow_parser_extended.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/flow_parser_extended.rs`:

```rust
use transcoderr::flow::{parse_flow, Node};

#[test]
fn parses_conditional_and_return() {
    let yaml = r#"
name: cond
triggers:
  - radarr: [downloaded]
match:
  expr: file.size_gb > 1
steps:
  - id: probe
    use: probe
  - id: gate
    if: probe.video.codec == "hevc"
    then:
      - return: skipped
    else:
      - id: enc
        use: transcode
        with: { codec: x265 }
on_failure:
  - use: notify
    with: { channel: discord, template: "fail {{file.name}}" }
"#;
    let flow = parse_flow(yaml).unwrap();
    assert_eq!(flow.match_expr.as_deref(), Some("file.size_gb > 1"));
    assert!(flow.on_failure.is_some());
    assert_eq!(flow.steps.len(), 2);
    match &flow.steps[1] {
        Node::Conditional { if_, then_, else_, .. } => {
            assert_eq!(if_, "probe.video.codec == \"hevc\"");
            assert_eq!(then_.len(), 1);
            assert!(matches!(then_[0], Node::Return { .. }));
            assert_eq!(else_.as_ref().unwrap().len(), 1);
        }
        _ => panic!("expected conditional"),
    }
}
```

- [ ] **Step 2: Extend the AST**

Replace `src/flow/model.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Flow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub match_expr: Option<String>,
    #[serde(default)]
    pub concurrency: Option<u32>,
    pub steps: Vec<Node>,
    #[serde(default)]
    pub on_failure: Option<Vec<Node>>,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Radarr(Vec<String>),
    Sonarr(Vec<String>),
    Lidarr(Vec<String>),
    Webhook(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Node {
    Conditional {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "if")]
        if_: String,
        #[serde(rename = "then")]
        then_: Vec<Node>,
        #[serde(rename = "else", default)]
        else_: Option<Vec<Node>>,
    },
    Return {
        #[serde(rename = "return")]
        return_: String,
    },
    Step {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "use")]
        use_: String,
        #[serde(default)]
        with: BTreeMap<String, Value>,
        #[serde(default)]
        retry: Option<Retry>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Retry {
    pub max: u32,
    #[serde(default)]
    pub on: Option<String>,
}
```

Note: serde with `untagged` enum requires that variants are uniquely structured. `Step` has `use:`, `Conditional` has `if:`, `Return` has `return:`. Distinct enough.

The Phase 1 `Step` struct usage broke. Update Phase 1 callers:
- `src/flow/parser.rs`: walk `steps` recursively to validate `use:` keys.
- `src/flow/engine.rs`: switch from `flow.steps.iter()` to a recursive walker (covered in Task 5).

- [ ] **Step 3: Update parser to walk recursively**

Replace `src/flow/parser.rs`:

```rust
use super::model::{Flow, Node};

const KNOWN_STEPS: &[&str] = &[
    "probe", "transcode", "output",
    // Phase 2 built-ins (added in later tasks):
    "verify.playable", "remux", "extract.subs", "strip.tracks",
    "move", "copy", "delete", "notify", "shell",
];

pub fn parse_flow(yaml: &str) -> anyhow::Result<Flow> {
    let flow: Flow = serde_yaml::from_str(yaml)?;
    validate(&flow)?;
    Ok(flow)
}

fn validate(flow: &Flow) -> anyhow::Result<()> {
    if flow.triggers.is_empty() {
        anyhow::bail!("flow {:?} has no triggers", flow.name);
    }
    walk(&flow.steps)?;
    if let Some(of) = &flow.on_failure { walk(of)?; }
    Ok(())
}

fn walk(nodes: &[Node]) -> anyhow::Result<()> {
    for n in nodes {
        match n {
            Node::Step { use_, .. } => {
                // Plugin steps may have any name — we only warn for completely unknown
                // names at compile-time. Final validation happens at run time when the
                // plugin registry is consulted. For now, accept anything.
                let _ = use_;
            }
            Node::Conditional { then_, else_, .. } => {
                walk(then_)?;
                if let Some(e) = else_ { walk(e)?; }
            }
            Node::Return { .. } => {}
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn known_step(use_: &str) -> bool { KNOWN_STEPS.contains(&use_) }
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`

Expect: existing Phase 1 tests still pass (because Phase 1 used `Step { use_, with }` and our `Node::Step` keeps the same field names). The new test passes too.

If tests fail because Phase 1 directly accessed `flow.steps[i].use_`, fix the call sites by pattern-matching `Node::Step { use_, .. }` first. Update accordingly.

- [ ] **Step 5: Commit**

```bash
git add src/flow/model.rs src/flow/parser.rs tests/flow_parser_extended.rs
git commit -m "feat(flow): conditional, return, retry, on_failure, match.expr in AST"
```

---

### Task 4: CEL expression evaluator wrapper

**Files:**
- Create: `src/flow/expr.rs`
- Modify: `src/flow/mod.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Add CEL crate**

Add to `[dependencies]` in `Cargo.toml`:

```toml
cel-interpreter = "0.9"
```

- [ ] **Step 2: Write the failing test**

Create `src/flow/expr.rs`:

```rust
use crate::flow::Context;
use cel_interpreter::{Context as CelCtx, Program, Value as CelValue};
use serde_json::Value;

pub fn eval_bool(expr: &str, ctx: &Context) -> anyhow::Result<bool> {
    let program = Program::compile(expr).map_err(|e| anyhow::anyhow!("compile: {e:?}"))?;
    let mut cel = CelCtx::default();
    bind_context(&mut cel, ctx);
    let v = program.execute(&cel).map_err(|e| anyhow::anyhow!("exec: {e:?}"))?;
    Ok(matches!(v, CelValue::Bool(true)))
}

pub fn eval_string_template(template: &str, ctx: &Context) -> anyhow::Result<String> {
    // Template is: literal text with {{ expr }} placeholders.
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i+1] == b'{' {
            let end = template[i+2..].find("}}").ok_or_else(|| anyhow::anyhow!("unterminated {{"))?;
            let expr = template[i+2..i+2+end].trim();
            let program = Program::compile(expr).map_err(|e| anyhow::anyhow!("compile: {e:?}"))?;
            let mut cel = CelCtx::default();
            bind_context(&mut cel, ctx);
            let v = program.execute(&cel).map_err(|e| anyhow::anyhow!("exec: {e:?}"))?;
            out.push_str(&format_cel(&v));
            i = i + 2 + end + 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Ok(out)
}

fn bind_context(cel: &mut CelCtx, ctx: &Context) {
    let v = serde_json::to_value(ctx).unwrap_or(Value::Null);
    if let Value::Object(map) = v {
        for (k, vv) in map {
            cel.add_variable(k, vv).ok();
        }
    }
}

fn format_cel(v: &CelValue) -> String {
    match v {
        CelValue::String(s) => s.to_string(),
        CelValue::Int(i) => i.to_string(),
        CelValue::UInt(u) => u.to_string(),
        CelValue::Float(f) => f.to_string(),
        CelValue::Bool(b) => b.to_string(),
        CelValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates_bool_expression_against_context() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        ctx.probe = Some(json!({ "video": { "codec": "h264" } }));
        assert!(eval_bool("probe.video.codec == \"h264\"", &ctx).unwrap());
        assert!(!eval_bool("probe.video.codec == \"hevc\"", &ctx).unwrap());
    }

    #[test]
    fn interpolates_template() {
        let ctx = Context::for_file("/m/Dune.mkv");
        let s = eval_string_template("file is {{ file.path }}", &ctx).unwrap();
        assert_eq!(s, "file is /m/Dune.mkv");
    }
}
```

Add to `src/flow/mod.rs`:

```rust
pub mod expr;
```

- [ ] **Step 3: Run**

Run: `cargo test flow::expr`
Expected: PASS. (Note: `cel-interpreter` API may vary — if compile errors, consult docs and adjust the `Value` mapping; the test contract stays the same.)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/flow/expr.rs src/flow/mod.rs
git commit -m "feat(flow): CEL expression and template evaluator"
```

---

### Task 5: Engine — recursive node executor with conditionals, return, retry, on_failure

**Files:**
- Modify: `src/flow/engine.rs`
- Create: `tests/flow_conditional.rs`
- Create: `tests/flow_return.rs`
- Create: `tests/flow_on_failure.rs`
- Create: `tests/flow_retry.rs`

- [ ] **Step 1: Replace the engine with a recursive walker**

Replace `src/flow/engine.rs`:

```rust
use crate::db;
use crate::flow::{expr, Context, Flow, Node};
use crate::steps::{registry::resolve, StepProgress};
use serde_json::json;
use sqlx::SqlitePool;

pub struct Engine {
    pool: SqlitePool,
}

#[derive(Debug)]
pub struct Outcome {
    pub status: String,
    pub label: Option<String>,
}

#[derive(Debug)]
struct StepIndex(u32);

impl Engine {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    pub async fn run(&self, flow: &Flow, job_id: i64, mut ctx: Context) -> anyhow::Result<Outcome> {
        // Resume.
        let resume = match db::checkpoints::get(&self.pool, job_id).await? {
            Some((idx, snap)) => {
                ctx = Context::from_snapshot(&snap)?;
                Some(idx as u32 + 1)
            }
            None => None,
        };

        let mut counter = 0u32;
        match self.run_nodes(&flow.steps, job_id, &mut ctx, &mut counter, resume).await {
            Ok(NodeOutcome::Continue) => Ok(Outcome { status: "completed".into(), label: None }),
            Ok(NodeOutcome::Return(label)) => Ok(Outcome { status: "skipped".into(), label: Some(label) }),
            Err(e) => {
                db::run_events::append(&self.pool, job_id, None, "failed",
                    Some(&json!({ "error": e.to_string() }))).await?;
                if let Some(of) = &flow.on_failure {
                    // Run failure handler with a small ctx extension.
                    let mut counter2 = u32::MAX / 2; // distinct space, never checkpointed
                    let _ = self.run_nodes(of, job_id, &mut ctx, &mut counter2, None).await;
                }
                Ok(Outcome { status: "failed".into(), label: None })
            }
        }
    }

    async fn run_nodes(
        &self, nodes: &[Node], job_id: i64, ctx: &mut Context,
        counter: &mut u32, resume_at: Option<u32>,
    ) -> anyhow::Result<NodeOutcome> {
        for n in nodes {
            let my_index = *counter;
            *counter += 1;
            if let Some(skip_below) = resume_at {
                if my_index < skip_below { /* fast-forward */ continue; }
            }
            match n {
                Node::Step { id, use_, with, retry } => {
                    let step_id = id.clone().unwrap_or_else(|| format!("{use_}_{my_index}"));
                    let max_attempts = retry.as_ref().map(|r| r.max + 1).unwrap_or(1);
                    let mut last_err: Option<anyhow::Error> = None;
                    for attempt in 1..=max_attempts {
                        db::run_events::append(&self.pool, job_id, Some(&step_id), "started",
                            Some(&json!({ "use": use_, "attempt": attempt }))).await?;
                        let runner = resolve(use_).await
                            .ok_or_else(|| anyhow::anyhow!("unknown step `use:` {}", use_))?;
                        let pool = self.pool.clone();
                        let step_id_for_cb = step_id.clone();
                        let mut cb = move |ev: StepProgress| {
                            let pool = pool.clone();
                            let step_id = step_id_for_cb.clone();
                            tokio::spawn(async move {
                                let (kind, payload) = match ev {
                                    StepProgress::Pct(p) => ("progress", json!({ "pct": p })),
                                    StepProgress::Log(l) => ("log", json!({ "msg": l })),
                                };
                                let _ = db::run_events::append(&pool, job_id, Some(&step_id), kind, Some(&payload)).await;
                            });
                        };
                        match runner.execute(with, ctx, &mut cb).await {
                            Ok(()) => {
                                db::run_events::append(&self.pool, job_id, Some(&step_id), "completed", None).await?;
                                db::checkpoints::upsert(&self.pool, job_id, my_index as i64, &ctx.to_snapshot()).await?;
                                last_err = None;
                                break;
                            }
                            Err(e) => {
                                db::run_events::append(&self.pool, job_id, Some(&step_id), "failed",
                                    Some(&json!({ "error": e.to_string(), "attempt": attempt }))).await?;
                                let should_retry = retry.as_ref().and_then(|r| r.on.as_deref())
                                    .map(|on_expr| expr::eval_bool(on_expr, ctx).unwrap_or(true))
                                    .unwrap_or(true);
                                if !should_retry || attempt == max_attempts {
                                    last_err = Some(e);
                                    break;
                                }
                                last_err = Some(e);
                            }
                        }
                    }
                    if let Some(e) = last_err { return Err(e); }
                }
                Node::Conditional { id, if_, then_, else_ } => {
                    let step_id = id.clone().unwrap_or_else(|| format!("if_{my_index}"));
                    let v = expr::eval_bool(if_, ctx)?;
                    db::run_events::append(&self.pool, job_id, Some(&step_id), "condition_evaluated",
                        Some(&json!({ "expr": if_, "result": v }))).await?;
                    let branch = if v { then_.as_slice() } else { else_.as_deref().unwrap_or(&[]) };
                    let outcome = Box::pin(self.run_nodes(branch, job_id, ctx, counter, resume_at)).await?;
                    if let NodeOutcome::Return(_) = &outcome { return Ok(outcome); }
                }
                Node::Return { return_ } => {
                    db::run_events::append(&self.pool, job_id, None, "returned",
                        Some(&json!({ "label": return_ }))).await?;
                    return Ok(NodeOutcome::Return(return_.clone()));
                }
            }
        }
        Ok(NodeOutcome::Continue)
    }
}

#[derive(Debug)]
enum NodeOutcome {
    Continue,
    Return(String),
}
```

Note: this references `crate::steps::registry::resolve`, which we'll add in Task 7. Keep this in mind — the next two tasks build that out.

- [ ] **Step 2: Conditional integration test**

Create `tests/flow_conditional.rs`:

```rust
use serde_json::json;
use tempfile::tempdir;
use transcoderr::{db, flow::{parse_flow, Context, Engine}};

#[tokio::test]
async fn conditional_then_branch_runs() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let yaml = r#"
name: c
triggers: [{ radarr: [downloaded] }]
steps:
  - id: gate
    if: file.path == "/m/x.mkv"
    then:
      - return: matched
    else:
      - return: missed
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "c", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", "/m/x.mkv", "{}").await.unwrap();
    let _ = db::jobs::claim_next(&pool).await.unwrap().unwrap();
    let outcome = Engine::new(pool.clone()).run(&flow, job_id, Context::for_file("/m/x.mkv")).await.unwrap();
    assert_eq!(outcome.status, "skipped");
    assert_eq!(outcome.label.as_deref(), Some("matched"));
}
```

- [ ] **Step 3: on_failure integration test**

Create `tests/flow_on_failure.rs`:

```rust
use tempfile::tempdir;
use transcoderr::{db, flow::{parse_flow, Context, Engine}};

#[tokio::test]
async fn on_failure_handler_runs() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let yaml = r#"
name: f
triggers: [{ radarr: [downloaded] }]
steps:
  - use: probe       # will fail because file doesn't exist
on_failure:
  - use: shell
    with: { cmd: "echo handler ran" }
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "f", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", "/no/such/file.mkv", "{}").await.unwrap();
    let _ = db::jobs::claim_next(&pool).await.unwrap().unwrap();
    let outcome = Engine::new(pool.clone()).run(&flow, job_id, Context::for_file("/no/such/file.mkv")).await.unwrap();
    assert_eq!(outcome.status, "failed");

    // Ensure the on_failure shell event recorded
    let evts: Vec<(String,)> = sqlx::query_as("SELECT step_id FROM run_events WHERE job_id = ? AND kind = 'completed'")
        .bind(job_id).fetch_all(&pool).await.unwrap();
    assert!(evts.iter().any(|(s,)| s.starts_with("shell_")), "shell handler should complete");
}
```

(`shell` step is implemented in Task 9; until then this test won't compile-time fail but will runtime fail. Mark with `#[ignore]` for now and remove after Task 9.)

- [ ] **Step 4: Build (engine compiles only after Tasks 6-7 add the registry)**

Skip running for now. Run: `cargo build`
Expected: failure on missing `crate::steps::registry::resolve` — that's fine, Tasks 6-7 add it.

- [ ] **Step 5: Commit**

```bash
git add src/flow/engine.rs tests/flow_conditional.rs tests/flow_on_failure.rs
git commit -m "feat(flow): recursive engine with conditionals, return, retry, on_failure"
```

---

### Task 6: Plugin manifest + discovery

**Files:**
- Create: `src/plugins/mod.rs`
- Create: `src/plugins/manifest.rs`
- Modify: `src/lib.rs`
- Create: `tests/plugin_discovery.rs`

- [ ] **Step 1: Manifest parser**

Create `src/plugins/manifest.rs`:

```rust
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub kind: String,                // "subprocess" or "builtin"
    pub entrypoint: Option<String>,  // required for subprocess
    pub provides_steps: Vec<String>,
    #[serde(default)]
    pub requires: serde_json::Value,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub manifest: Manifest,
    pub manifest_dir: PathBuf,
    pub schema: serde_json::Value,
}

pub fn load_from_dir(dir: &Path) -> anyhow::Result<DiscoveredPlugin> {
    let manifest_path = dir.join("manifest.toml");
    let raw = std::fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = toml::from_str(&raw)?;
    let schema_path = dir.join("schema.json");
    let schema: serde_json::Value = if schema_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&schema_path)?)?
    } else {
        serde_json::json!({})
    };
    Ok(DiscoveredPlugin { manifest, manifest_dir: dir.to_path_buf(), schema })
}
```

- [ ] **Step 2: Discovery scanner**

Create `src/plugins/mod.rs`:

```rust
pub mod manifest;
pub mod subprocess;

use manifest::{DiscoveredPlugin, load_from_dir};
use std::path::Path;

pub fn discover(plugins_dir: &Path) -> anyhow::Result<Vec<DiscoveredPlugin>> {
    if !plugins_dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(plugins_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() { continue; }
        if !path.join("manifest.toml").exists() { continue; }
        match load_from_dir(&path) {
            Ok(p) => out.push(p),
            Err(e) => tracing::warn!(?path, error = %e, "skipping invalid plugin"),
        }
    }
    Ok(out)
}
```

Stub `src/plugins/subprocess.rs`:

```rust
pub struct SubprocessStep;
```

Add to `src/lib.rs`:

```rust
pub mod plugins;
```

- [ ] **Step 3: Discovery test with fixture**

Create `tests/fixtures/plugins/hello/manifest.toml`:

```toml
name = "hello"
version = "0.1.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["hello"]
```

Create `tests/fixtures/plugins/hello/schema.json`:

```json
{
  "type": "object",
  "properties": { "greeting": { "type": "string" } }
}
```

Create `tests/fixtures/plugins/hello/bin/run` (stub for now, made executable in Task 7):

```bash
#!/usr/bin/env bash
echo '{"event":"result","status":"err","error":{"code":"unimplemented","msg":"step trait not yet wired"}}'
```

Create `tests/plugin_discovery.rs`:

```rust
use transcoderr::plugins::discover;

#[test]
fn discovers_hello_plugin() {
    let dir = std::path::Path::new("tests/fixtures/plugins");
    let plugins = discover(dir).unwrap();
    let hello = plugins.iter().find(|p| p.manifest.name == "hello").expect("hello discovered");
    assert_eq!(hello.manifest.provides_steps, vec!["hello"]);
    assert_eq!(hello.schema["properties"]["greeting"]["type"], "string");
}
```

- [ ] **Step 4: Run**

Run: `cargo test --test plugin_discovery`
Expected: PASS.

Make `tests/fixtures/plugins/hello/bin/run` executable: `chmod +x tests/fixtures/plugins/hello/bin/run`.

- [ ] **Step 5: Commit**

```bash
git add src/plugins/ src/lib.rs tests/plugin_discovery.rs tests/fixtures/
git commit -m "feat(plugins): manifest schema + discovery"
```

---

### Task 7: Subprocess plugin Step impl + step registry

**Files:**
- Replace: `src/plugins/subprocess.rs`
- Create: `src/steps/registry.rs`
- Modify: `src/steps/mod.rs`
- Create: `tests/plugin_subprocess.rs`
- Modify: `tests/fixtures/plugins/hello/bin/run`

- [ ] **Step 1: Subprocess step impl**

Replace `src/plugins/subprocess.rs`:

```rust
use crate::flow::Context;
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct SubprocessStep {
    pub step_name: String,
    pub entrypoint_abs: PathBuf,
}

#[async_trait]
impl Step for SubprocessStep {
    fn name(&self) -> &'static str {
        // Hack: we leak a static — but step names are stable strings the host
        // controls, so the leak is bounded.
        Box::leak(self.step_name.clone().into_boxed_str())
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let mut child = Command::new(&self.entrypoint_abs)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();

        // init
        let init = json!({ "method": "init", "params": { "workdir": "." } });
        stdin.write_all(format!("{init}\n").as_bytes()).await?;

        // execute
        let exec = json!({ "method": "execute", "params": {
            "step_id": self.step_name,
            "with": with,
            "context": ctx,
        }});
        stdin.write_all(format!("{exec}\n").as_bytes()).await?;

        let mut step_result: Option<Value> = None;
        while let Ok(Some(line)) = stdout.next_line().await {
            let v: Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
            match v["event"].as_str() {
                Some("progress") => {
                    if let Some(p) = v["pct"].as_f64() { on_progress(StepProgress::Pct(p)); }
                }
                Some("log") => {
                    if let Some(m) = v["msg"].as_str() { on_progress(StepProgress::Log(m.into())); }
                }
                Some("context_set") => {
                    if let (Some(k), Some(val)) = (v["key"].as_str(), v.get("value")) {
                        ctx.steps.insert(k.into(), val.clone());
                    }
                }
                Some("result") => { step_result = Some(v); break; }
                _ => {}
            }
        }
        let _ = stdin.shutdown().await;
        let _ = child.wait().await;
        let res = step_result.ok_or_else(|| anyhow::anyhow!("plugin {} produced no result", self.step_name))?;
        if res["status"] == "ok" {
            Ok(())
        } else {
            anyhow::bail!("plugin {} failed: {}", self.step_name, res["error"]["msg"])
        }
    }
}
```

- [ ] **Step 2: Step registry**

Create `src/steps/registry.rs`:

```rust
use crate::plugins::manifest::DiscoveredPlugin;
use crate::plugins::subprocess::SubprocessStep;
use crate::steps::{builtin, Step};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::OnceCell;

static REGISTRY: OnceCell<Arc<Registry>> = OnceCell::const_new();

pub struct Registry {
    by_name: HashMap<String, Arc<dyn Step>>,
}

impl Registry {
    pub fn empty() -> Self { Self { by_name: HashMap::new() } }
}

pub async fn init(discovered: Vec<DiscoveredPlugin>) {
    let mut reg = Registry::empty();
    builtin::register_all(&mut reg.by_name);
    for d in discovered {
        if d.manifest.kind != "subprocess" { continue; }
        let entry = d.manifest.entrypoint.clone().unwrap_or_default();
        let abs = d.manifest_dir.join(&entry);
        for step_name in &d.manifest.provides_steps {
            let step = SubprocessStep { step_name: step_name.clone(), entrypoint_abs: abs.clone() };
            reg.by_name.insert(step_name.clone(), Arc::new(step));
        }
    }
    let _ = REGISTRY.set(Arc::new(reg));
}

pub async fn resolve(name: &str) -> Option<Arc<dyn Step>> {
    let reg = REGISTRY.get()?;
    reg.by_name.get(name).cloned()
}
```

- [ ] **Step 3: Built-in registration helper**

Create `src/steps/builtin.rs`:

```rust
use crate::steps::{output::OutputStep, probe::ProbeStep, transcode::TranscodeStep, Step};
use std::collections::HashMap;
use std::sync::Arc;

pub fn register_all(map: &mut HashMap<String, Arc<dyn Step>>) {
    map.insert("probe".into(), Arc::new(ProbeStep));
    map.insert("transcode".into(), Arc::new(TranscodeStep));
    map.insert("output".into(), Arc::new(OutputStep));
    // verify.playable, remux, …, registered when their files are added (Tasks 8-9)
}
```

Update `src/steps/mod.rs` — keep `Step` trait + `StepProgress`, and add:

```rust
pub mod builtin;
pub mod output;
pub mod probe;
pub mod registry;
pub mod transcode;
```

(Replace the old `pub fn dispatch` from Phase 1 — engine now uses `registry::resolve` instead.)

- [ ] **Step 4: Subprocess plugin contract test**

Replace `tests/fixtures/plugins/hello/bin/run` with:

```bash
#!/usr/bin/env bash
# Tiny test plugin: reads JSON-RPC, replies with success and one progress event.
read INIT_LINE
read EXEC_LINE
echo '{"event":"progress","pct":50.0}'
echo '{"event":"context_set","key":"hello","value":{"greeted":true}}'
echo '{"event":"result","status":"ok","outputs":{}}'
```

Make sure it's executable: `chmod +x tests/fixtures/plugins/hello/bin/run`.

Create `tests/plugin_subprocess.rs`:

```rust
use std::collections::BTreeMap;
use transcoderr::flow::Context;
use transcoderr::plugins::{discover, subprocess::SubprocessStep};
use transcoderr::steps::{Step, StepProgress};

#[tokio::test]
async fn subprocess_plugin_round_trip() {
    let plugins = discover(std::path::Path::new("tests/fixtures/plugins")).unwrap();
    let p = plugins.iter().find(|p| p.manifest.name == "hello").unwrap();
    let entrypoint = p.manifest.entrypoint.clone().unwrap();
    let abs = p.manifest_dir.join(&entrypoint);
    let step = SubprocessStep { step_name: "hello".into(), entrypoint_abs: abs };

    let mut ctx = Context::for_file("/tmp/x");
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    step.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    assert!(events.iter().any(|e| matches!(e, StepProgress::Pct(p) if (*p - 50.0).abs() < 0.01)));
    assert_eq!(ctx.steps.get("hello").unwrap()["greeted"], true);
}
```

- [ ] **Step 5: Run**

Run: `cargo test --test plugin_subprocess`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/plugins/subprocess.rs src/steps/ tests/plugin_subprocess.rs tests/fixtures/plugins/hello/bin/run
git commit -m "feat(plugins): subprocess Step impl and step registry"
```

---

### Task 8: Built-in steps — verify.playable, remux, extract.subs, strip.tracks

**Files:**
- Create: `src/steps/verify_playable.rs`
- Create: `src/steps/remux.rs`
- Create: `src/steps/extract_subs.rs`
- Create: `src/steps/strip_tracks.rs`
- Modify: `src/steps/builtin.rs`
- Create: `tests/step_verify_playable.rs`
- Create: `tests/step_remux.rs`

- [ ] **Step 1: verify.playable test**

Create `tests/step_verify_playable.rs`:

```rust
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{verify_playable::VerifyPlayableStep, Step, StepProgress};

#[tokio::test]
async fn verify_playable_succeeds_on_good_file() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("ok.mkv");
    make_testsrc_mkv(&p, 2).await.unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    // Pretend a prior probe set duration.
    ctx.probe = Some(serde_json::json!({ "format": { "duration": "2.000000" }}));
    ctx.steps.insert("transcode".into(), serde_json::json!({ "output_path": p.to_string_lossy() }));
    let mut events = vec![];
    let mut cb = |e: StepProgress| events.push(e);
    VerifyPlayableStep.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
}

#[tokio::test]
async fn verify_playable_fails_on_truncated_output() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("bad.mkv");
    std::fs::write(&p, b"not a real mkv").unwrap();
    let mut ctx = Context::for_file(p.to_string_lossy());
    ctx.probe = Some(serde_json::json!({ "format": { "duration": "10.000000" }}));
    ctx.steps.insert("transcode".into(), serde_json::json!({ "output_path": p.to_string_lossy() }));
    let mut cb = |_: StepProgress| {};
    let err = VerifyPlayableStep.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap_err();
    assert!(err.to_string().contains("verify"));
}
```

- [ ] **Step 2: verify.playable impl**

Create `src/steps/verify_playable.rs`:

```rust
use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct VerifyPlayableStep;

#[async_trait]
impl Step for VerifyPlayableStep {
    fn name(&self) -> &'static str { "verify.playable" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let min_ratio = with.get("min_duration_ratio").and_then(|v| v.as_f64()).unwrap_or(0.99);

        let target = ctx.steps.get("transcode")
            .and_then(|v| v.get("output_path"))
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.file.path)
            .to_string();

        on_progress(StepProgress::Log(format!("verifying {target}")));
        let probed = ffprobe_json(Path::new(&target)).await
            .map_err(|e| anyhow::anyhow!("verify ffprobe failed: {e}"))?;

        let original_dur = ctx.probe.as_ref()
            .and_then(|p| p["format"]["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let new_dur = probed["format"]["duration"].as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if original_dur > 0.0 && (new_dur / original_dur) < min_ratio {
            anyhow::bail!("verify failed: new={new_dur:.2}s vs original={original_dur:.2}s (<{:.2}x)", min_ratio);
        }
        Ok(())
    }
}
```

- [ ] **Step 3: remux test + impl**

Create `tests/step_remux.rs`:

```rust
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempfile::tempdir;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::Context;
use transcoderr::steps::{remux::RemuxStep, Step, StepProgress};

#[tokio::test]
async fn remux_changes_container_only() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("in.mkv");
    make_testsrc_mkv(&src, 1).await.unwrap();
    let mut ctx = Context::for_file(src.to_string_lossy());
    let mut with: BTreeMap<String, Value> = BTreeMap::new();
    with.insert("container".into(), json!("mp4"));
    let mut cb = |_: StepProgress| {};
    RemuxStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
    let out = ctx.steps.get("transcode").unwrap()["output_path"].as_str().unwrap();
    assert!(out.ends_with(".transcoderr.tmp.mp4"));
}
```

Create `src/steps/remux.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct RemuxStep;

#[async_trait]
impl Step for RemuxStep {
    fn name(&self) -> &'static str { "remux" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let container = with.get("container").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("remux: missing `container`"))?;
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension(format!("transcoderr.tmp.{container}"));
        let _ = std::fs::remove_file(&dest);
        on_progress(StepProgress::Log(format!("remux → {}", dest.display())));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-y", "-i"]).arg(&src)
            .args(["-c", "copy"]).arg(&dest)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().await?;
        if !status.success() { anyhow::bail!("remux ffmpeg failed"); }
        ctx.record_step_output("transcode", json!({ "output_path": dest.to_string_lossy() }));
        Ok(())
    }
}
```

- [ ] **Step 4: extract.subs and strip.tracks (terse impls)**

Create `src/steps/extract_subs.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct ExtractSubsStep;

#[async_trait]
impl Step for ExtractSubsStep {
    fn name(&self) -> &'static str { "extract.subs" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let lang = with.get("language").and_then(|v| v.as_str()).unwrap_or("eng");
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension(format!("{lang}.srt"));
        on_progress(StepProgress::Log(format!("extracting {lang} subs → {}", dest.display())));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-y", "-i"]).arg(&src)
            .args(["-map", &format!("0:s:m:language:{lang}?")])
            .arg(&dest)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().await?;
        if !status.success() { anyhow::bail!("extract.subs ffmpeg failed"); }
        Ok(())
    }
}
```

Create `src/steps/strip_tracks.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Strip tracks not matching configured `keep` languages or `keep_video=1`/`keep_audio=1`.
pub struct StripTracksStep;

#[async_trait]
impl Step for StripTracksStep {
    fn name(&self) -> &'static str { "strip.tracks" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let langs = with.get("keep_audio_languages")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
            .unwrap_or_else(|| vec!["eng".into()]);
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension("transcoderr.tmp.mkv");
        let _ = std::fs::remove_file(&dest);

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-y", "-i"]).arg(&src);
        cmd.args(["-map", "0:v", "-c:v", "copy"]);
        for l in &langs {
            cmd.args(["-map", &format!("0:a:m:language:{l}?"), "-c:a", "copy"]);
        }
        cmd.args(["-map", "0:s?", "-c:s", "copy"]).arg(&dest);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        on_progress(StepProgress::Log(format!("strip tracks: keep audio {langs:?}")));
        let status = cmd.status().await?;
        if !status.success() { anyhow::bail!("strip.tracks ffmpeg failed"); }
        ctx.record_step_output("transcode", json!({ "output_path": dest.to_string_lossy() }));
        Ok(())
    }
}
```

- [ ] **Step 5: Register in builtin.rs**

Replace `src/steps/builtin.rs`:

```rust
use crate::steps::{
    extract_subs::ExtractSubsStep, output::OutputStep, probe::ProbeStep,
    remux::RemuxStep, strip_tracks::StripTracksStep, transcode::TranscodeStep,
    verify_playable::VerifyPlayableStep, Step,
};
use std::collections::HashMap;
use std::sync::Arc;

pub fn register_all(map: &mut HashMap<String, Arc<dyn Step>>) {
    map.insert("probe".into(),           Arc::new(ProbeStep));
    map.insert("transcode".into(),       Arc::new(TranscodeStep));
    map.insert("output".into(),          Arc::new(OutputStep));
    map.insert("verify.playable".into(), Arc::new(VerifyPlayableStep));
    map.insert("remux".into(),           Arc::new(RemuxStep));
    map.insert("extract.subs".into(),    Arc::new(ExtractSubsStep));
    map.insert("strip.tracks".into(),    Arc::new(StripTracksStep));
}
```

Update `src/steps/mod.rs`:

```rust
pub mod builtin;
pub mod extract_subs;
pub mod output;
pub mod probe;
pub mod registry;
pub mod remux;
pub mod strip_tracks;
pub mod transcode;
pub mod verify_playable;
```

- [ ] **Step 6: Run all step tests**

Run: `cargo test --tests`
Expected: all step tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/steps/ tests/step_verify_playable.rs tests/step_remux.rs
git commit -m "feat(steps): verify.playable, remux, extract.subs, strip.tracks"
```

---

### Task 9: File-management + escape-hatch built-ins (move, copy, delete, shell)

**Files:**
- Create: `src/steps/move_step.rs`
- Create: `src/steps/copy_step.rs`
- Create: `src/steps/delete_step.rs`
- Create: `src/steps/shell.rs`
- Modify: `src/steps/builtin.rs` and `src/steps/mod.rs`
- Create: `tests/step_filesys.rs`

- [ ] **Step 1: Implementations**

Create `src/steps/move_step.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct MoveStep;

#[async_trait]
impl Step for MoveStep {
    fn name(&self) -> &'static str { "move" }

    async fn execute(
        &self, with: &BTreeMap<String, Value>, ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let dest = with.get("to").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("move: missing `to`"))?;
        let src = std::path::Path::new(&ctx.file.path);
        let dest_path = std::path::Path::new(dest).join(src.file_name().unwrap_or_default());
        if let Some(parent) = dest_path.parent() { std::fs::create_dir_all(parent)?; }
        on_progress(StepProgress::Log(format!("move {} -> {}", src.display(), dest_path.display())));
        std::fs::rename(src, &dest_path).or_else(|_| {
            std::fs::copy(src, &dest_path)?; std::fs::remove_file(src)?; Ok::<_, std::io::Error>(())
        })?;
        ctx.file.path = dest_path.to_string_lossy().to_string();
        Ok(())
    }
}
```

Create `src/steps/copy_step.rs` — same shape but copies, doesn't update `ctx.file.path`.

Create `src/steps/delete_step.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct DeleteStep;

#[async_trait]
impl Step for DeleteStep {
    fn name(&self) -> &'static str { "delete" }

    async fn execute(
        &self, _with: &BTreeMap<String, Value>, ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let p = std::path::Path::new(&ctx.file.path);
        on_progress(StepProgress::Log(format!("delete {}", p.display())));
        std::fs::remove_file(p)?;
        Ok(())
    }
}
```

Create `src/steps/shell.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::{expr, Context};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use tokio::process::Command;

pub struct ShellStep;

#[async_trait]
impl Step for ShellStep {
    fn name(&self) -> &'static str { "shell" }

    async fn execute(
        &self, with: &BTreeMap<String, Value>, ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let cmd_template = with.get("cmd").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("shell: missing `cmd`"))?;
        let cmd = expr::eval_string_template(cmd_template, ctx)?;
        on_progress(StepProgress::Log(format!("$ {cmd}")));
        let status = Command::new("sh").arg("-c").arg(&cmd)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().await?;
        if !status.success() { anyhow::bail!("shell exited {:?}", status.code()); }
        Ok(())
    }
}
```

- [ ] **Step 2: Register**

Append to `src/steps/builtin.rs` registry:

```rust
    map.insert("move".into(),   Arc::new(MoveStep));
    map.insert("copy".into(),   Arc::new(CopyStep));
    map.insert("delete".into(), Arc::new(DeleteStep));
    map.insert("shell".into(),  Arc::new(ShellStep));
```

(Add `use` lines for the new modules at the top.)

Update `src/steps/mod.rs`:

```rust
pub mod copy_step;
pub mod delete_step;
pub mod move_step;
pub mod shell;
```

- [ ] **Step 3: Test**

Create `tests/step_filesys.rs` with one test per step (move, copy, delete, shell). Pattern follows the prior step tests — create a tempfile, run the step, assert the side effect.

(For brevity I'm not pasting all four — same shape as `step_output.rs` from Phase 1.)

Run: `cargo test --test step_filesys`
Expected: all PASS.

Run: `cargo test --test flow_on_failure` (now `shell` exists, this is no longer ignored).
Expected: PASS. Remove `#[ignore]` if present.

- [ ] **Step 4: Commit**

```bash
git add src/steps/ tests/step_filesys.rs
git commit -m "feat(steps): move, copy, delete, shell"
```

---

### Task 10: Notifiers — Discord, ntfy, generic webhook + `notify` step

**Files:**
- Create: `src/notifiers/mod.rs`
- Create: `src/notifiers/discord.rs`
- Create: `src/notifiers/ntfy.rs`
- Create: `src/notifiers/webhook.rs`
- Create: `src/db/notifiers.rs`
- Create: `src/steps/notify.rs`
- Modify: `src/steps/builtin.rs`, `src/steps/mod.rs`, `src/lib.rs`
- Modify: `Cargo.toml` (add `reqwest` to runtime deps)
- Create: `tests/notify_discord.rs`

- [ ] **Step 1: Move reqwest to runtime deps**

In `Cargo.toml`, move `reqwest` from `[dev-dependencies]` to `[dependencies]`:

```toml
[dependencies]
# ... existing ...
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 2: Notifier trait + dispatch**

Create `src/notifiers/mod.rs`:

```rust
pub mod discord;
pub mod ntfy;
pub mod webhook;

use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn send(&self, message: &str, extra: &Value) -> anyhow::Result<()>;
}

pub fn build(kind: &str, config: &Value) -> anyhow::Result<Box<dyn Notifier>> {
    match kind {
        "discord" => Ok(Box::new(discord::Discord::new(config)?)),
        "ntfy"    => Ok(Box::new(ntfy::Ntfy::new(config)?)),
        "webhook" => Ok(Box::new(webhook::WebhookNotifier::new(config)?)),
        other     => anyhow::bail!("unknown notifier kind {other}"),
    }
}
```

- [ ] **Step 3: Discord impl**

Create `src/notifiers/discord.rs`:

```rust
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
```

- [ ] **Step 4: ntfy + webhook (terse)**

Create `src/notifiers/ntfy.rs`:

```rust
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
```

Create `src/notifiers/webhook.rs`:

```rust
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
```

- [ ] **Step 5: Notifiers DB**

Create `src/db/notifiers.rs`:

```rust
use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NotifierRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub config_json: String,
}

pub async fn upsert(pool: &SqlitePool, name: &str, kind: &str, config: &Value) -> anyhow::Result<i64> {
    let cj = serde_json::to_string(config)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO notifiers (name, kind, config_json) VALUES (?, ?, ?)
         ON CONFLICT (name) DO UPDATE SET kind = excluded.kind, config_json = excluded.config_json
         RETURNING id"
    ).bind(name).bind(kind).bind(cj).fetch_one(pool).await?;
    Ok(id)
}

pub async fn get_by_name(pool: &SqlitePool, name: &str) -> anyhow::Result<Option<NotifierRow>> {
    Ok(sqlx::query_as("SELECT id, name, kind, config_json FROM notifiers WHERE name = ?")
        .bind(name).fetch_optional(pool).await?)
}
```

Add `pub mod notifiers;` to `src/db/mod.rs`.

- [ ] **Step 6: `notify` step**

Create `src/steps/notify.rs`:

```rust
use super::{Step, StepProgress};
use crate::db;
use crate::flow::{expr, Context};
use crate::notifiers;
use async_trait::async_trait;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::collections::BTreeMap;

pub struct NotifyStep {
    pub pool: SqlitePool,
}

#[async_trait]
impl Step for NotifyStep {
    fn name(&self) -> &'static str { "notify" }

    async fn execute(
        &self, with: &BTreeMap<String, Value>, ctx: &mut Context,
        on_progress: &mut dyn FnMut(StepProgress) + Send,
    ) -> anyhow::Result<()> {
        let channel = with.get("channel").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("notify: missing `channel`"))?;
        let template = with.get("template").and_then(|v| v.as_str()).unwrap_or("");
        let message = expr::eval_string_template(template, ctx)?;
        on_progress(StepProgress::Log(format!("notify {channel}: {message}")));
        let row = db::notifiers::get_by_name(&self.pool, channel).await?
            .ok_or_else(|| anyhow::anyhow!("notify: notifier {channel:?} not configured"))?;
        let cfg: Value = serde_json::from_str(&row.config_json)?;
        let notifier = notifiers::build(&row.kind, &cfg)?;
        notifier.send(&message, &json!({"file": ctx.file.path})).await?;
        Ok(())
    }
}
```

- [ ] **Step 7: Register `notify` (needs pool)**

The notify step needs the pool. Adjust `src/steps/registry.rs`'s `init` to receive the pool:

```rust
pub async fn init(pool: SqlitePool, discovered: Vec<DiscoveredPlugin>) {
    let mut reg = Registry::empty();
    builtin::register_all(&mut reg.by_name, pool.clone());
    // ... unchanged from here ...
}
```

Update `src/steps/builtin.rs` `register_all` to take `pool: SqlitePool` and pass it to `NotifyStep`. Update `src/steps/mod.rs` to add `pub mod notify;`. Add `use crate::steps::notify::NotifyStep;` and `map.insert("notify".into(), Arc::new(NotifyStep { pool: pool.clone() }));`.

Update `main.rs` to call `transcoderr::steps::registry::init(pool.clone(), plugins::discover(&plugins_dir)?).await;` before starting the worker.

- [ ] **Step 8: Discord notifier integration test**

Create `tests/notify_discord.rs`:

```rust
use serde_json::json;
use transcoderr::notifiers;

#[tokio::test]
async fn discord_posts_to_url() {
    // Spin a tiny mock server that captures the body.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let recv = received.clone();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::io::AsyncReadExt::read(&mut s, &mut buf).await.unwrap();
        *recv.lock().await = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tokio::io::AsyncWriteExt::write_all(&mut s, b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n").await;
    });

    let n = notifiers::build("discord", &json!({"url": format!("http://{addr}/x")})).unwrap();
    n.send("hello", &json!({})).await.unwrap();
    let body = received.lock().await.clone();
    assert!(body.contains("\"content\":\"hello\""));
}
```

Run: `cargo test --test notify_discord`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/notifiers/ src/db/notifiers.rs src/db/mod.rs src/steps/ src/main.rs src/lib.rs Cargo.toml tests/notify_discord.rs
git commit -m "feat(notifiers): Discord/ntfy/webhook + notify step"
```

---

### Task 11: Sonarr + Lidarr + Generic webhook adapters + dedup

**Files:**
- Create: `src/http/webhook_sonarr.rs`
- Create: `src/http/webhook_lidarr.rs`
- Create: `src/http/webhook_generic.rs`
- Create: `src/http/dedup.rs`
- Modify: `src/http/mod.rs`
- Modify: `src/db/flows.rs` (add `list_enabled_for_*`)
- Create: `tests/webhook_sonarr.rs`, `tests/webhook_generic.rs`, `tests/webhook_dedup.rs`

- [ ] **Step 1: Sonarr adapter**

Create `src/http/webhook_sonarr.rs`:

```rust
use crate::{db, http::AppState, http::dedup::DedupCache};
use axum::{extract::State, http::{HeaderMap, StatusCode}, Extension, Json};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "eventType")] event_type: String,
    #[serde(rename = "episodeFile")] episode_file: Option<EpisodeFile>,
}
#[derive(Debug, Deserialize)] struct EpisodeFile { path: String }

pub async fn handle(
    State(state): State<AppState>,
    Extension(dedup): Extension<Arc<DedupCache>>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    let source = db::sources::get_by_kind_and_token(&state.pool, "sonarr", token).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let payload: Payload = serde_json::from_value(raw.0.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let event = match payload.event_type.as_str() {
        "Download" | "EpisodeFileImport" => "downloaded",
        other => return Ok(StatusCode::ACCEPTED).map_err(|_:()| StatusCode::ACCEPTED).map(|_| StatusCode::ACCEPTED).or_else(|_| Ok(StatusCode::ACCEPTED)).map_err(|_:StatusCode| other).map_err(|_| StatusCode::ACCEPTED),  // accept-and-ignore
    };
    let Some(file) = payload.episode_file else { return Ok(StatusCode::ACCEPTED) };
    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    if !dedup.observe(source.id, &file.path, &raw_str) { return Ok(StatusCode::ACCEPTED); }
    let flows = db::flows::list_enabled_for_sonarr(&state.pool, event).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for flow in flows {
        let _ = db::jobs::insert_with_source(&state.pool, flow.id, flow.version, source.id, "sonarr", &file.path, &raw_str).await;
    }
    Ok(StatusCode::ACCEPTED)
}
```

(Lidarr is analogous — `trackFile.path`. Same structure.)

- [ ] **Step 2: Generic adapter**

Create `src/http/webhook_generic.rs`:

```rust
use crate::{db, flow::expr, http::AppState, http::dedup::DedupCache};
use axum::{extract::{Path, State}, http::{HeaderMap, StatusCode}, Extension, Json};
use serde_json::Value;
use std::sync::Arc;
use crate::flow::Context;

pub async fn handle(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Extension(dedup): Extension<Arc<DedupCache>>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    let source = db::sources::get_webhook_by_name_and_token(&state.pool, &name, token).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.ok_or(StatusCode::UNAUTHORIZED)?;
    let cfg: Value = serde_json::from_str(&source.config_json).unwrap_or(Value::Null);
    let path_expr = cfg["path_expr"].as_str().unwrap_or("payload.path");
    // Build a context with `payload` bound
    let mut ctx = Context::for_file("");
    ctx.steps.insert("payload".into(), raw.0.clone());
    let path = expr::eval_string_template(&format!("{{{{ {path_expr} }}}}"), &ctx)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    if !dedup.observe(source.id, &path, &raw_str) { return Ok(StatusCode::ACCEPTED); }
    let flows = db::flows::list_enabled_for_webhook(&state.pool, &name).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for flow in flows {
        let _ = db::jobs::insert_with_source(&state.pool, flow.id, flow.version, source.id, "webhook", &path, &raw_str).await;
    }
    Ok(StatusCode::ACCEPTED)
}
```

- [ ] **Step 3: Dedup cache**

Create `src/http/dedup.rs`:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct DedupCache {
    inner: Mutex<HashMap<String, Instant>>,
    window: Duration,
}

impl DedupCache {
    pub fn new(window: Duration) -> Self { Self { inner: Mutex::new(HashMap::new()), window } }

    /// Returns true if NEW (not a recent duplicate).
    pub fn observe(&self, source_id: i64, path: &str, raw_payload: &str) -> bool {
        let key = format!("{source_id}|{path}|{}", short_hash(raw_payload));
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        g.retain(|_, t| now.duration_since(*t) < self.window);
        match g.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => false,
            std::collections::hash_map::Entry::Vacant(v) => { v.insert(now); true }
        }
    }
}

fn short_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
```

- [ ] **Step 4: Update jobs CRUD + flows queries**

Append to `src/db/jobs.rs`:

```rust
pub async fn insert_with_source(
    pool: &SqlitePool,
    flow_id: i64, flow_version: i64, source_id: i64,
    source_kind: &str, file_path: &str, payload: &str,
) -> anyhow::Result<i64> {
    let now = now_unix();
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO jobs (flow_id, flow_version, source_id, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at)
         VALUES (?, ?, ?, ?, ?, ?, 'pending', 0, 0, ?) RETURNING id"
    ).bind(flow_id).bind(flow_version).bind(source_id).bind(source_kind)
     .bind(file_path).bind(payload).bind(now).fetch_one(pool).await?)
}
```

Append to `src/db/flows.rs` analogous `list_enabled_for_sonarr`, `list_enabled_for_lidarr`, `list_enabled_for_webhook(name: &str)` — each filters flows whose triggers match.

- [ ] **Step 5: Wire routes and dedup extension**

Replace `src/http/mod.rs`:

```rust
use crate::config::Config;
use axum::{routing::post, Extension, Router};
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};

pub mod dedup;
pub mod webhook_radarr;
pub mod webhook_sonarr;
pub mod webhook_lidarr;
pub mod webhook_generic;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    let dedup = Arc::new(dedup::DedupCache::new(Duration::from_secs(300)));
    Router::new()
        .route("/webhook/radarr",   post(webhook_radarr::handle))
        .route("/webhook/sonarr",   post(webhook_sonarr::handle))
        .route("/webhook/lidarr",   post(webhook_lidarr::handle))
        .route("/webhook/:name",    post(webhook_generic::handle))
        .layer(Extension(dedup))
        .with_state(state)
}
```

Adjust `webhook_radarr::handle` to pull token from `Authorization` header and look up the source row instead of using the bootstrap token (Phase 1 used the config-baked token; now it uses sources). Migrate the Phase 1 test by inserting a `radarr` source.

- [ ] **Step 6: Tests**

Create `tests/webhook_dedup.rs`:

```rust
use std::time::Duration;
use transcoderr::http::dedup::DedupCache;

#[test]
fn duplicate_within_window_rejected() {
    let c = DedupCache::new(Duration::from_secs(60));
    assert!(c.observe(1, "/m/x", r#"{"a":1}"#));
    assert!(!c.observe(1, "/m/x", r#"{"a":1}"#));
    assert!(c.observe(1, "/m/x", r#"{"a":2}"#));    // payload differs → new
    assert!(c.observe(2, "/m/x", r#"{"a":1}"#));    // different source → new
}
```

Create `tests/webhook_sonarr.rs` and `tests/webhook_generic.rs` mirroring the Phase 1 `webhook_to_complete` pattern but for the new adapters. Each test inserts a matching source row, posts a payload, asserts a job was created.

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add src/http/ src/db/ tests/webhook_sonarr.rs tests/webhook_generic.rs tests/webhook_dedup.rs
git commit -m "feat(http): Sonarr/Lidarr/generic webhook adapters with dedup"
```

---

### Task 12: Per-step timeout enforcement

**Files:**
- Modify: `src/flow/engine.rs`

- [ ] **Step 1: Wrap `runner.execute(...)` in `tokio::time::timeout`**

In the `Node::Step` arm of `run_nodes`, replace the `runner.execute(...)` call with:

```rust
let timeout_secs = with.get("timeout")
    .and_then(|v| v.as_u64())
    .unwrap_or_else(|| match use_.as_str() {
        "transcode" => 86_400,
        "probe" | "verify.playable" => 60,
        _ => 600,
    });
let exec = runner.execute(with, ctx, &mut cb);
let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), exec).await;
match result {
    Ok(Ok(())) => { /* success path as before */ }
    Ok(Err(e)) => { /* failure path as before */ }
    Err(_) => {
        db::run_events::append(&self.pool, job_id, Some(&step_id), "failed",
            Some(&json!({ "error": "timeout", "after_seconds": timeout_secs }))).await?;
        last_err = Some(anyhow::anyhow!("timeout after {timeout_secs}s"));
        break;
    }
}
```

(Only sketch shown — preserve the existing match arms; add the `Err(_)` branch for timeout.)

- [ ] **Step 2: Test**

Append a test to `tests/flow_retry.rs` using `shell` with `cmd: "sleep 5"` and `timeout: 1` — asserts the step fails with a timeout-ish error within ~2s.

Run: `cargo test --test flow_retry`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/flow/engine.rs tests/flow_retry.rs
git commit -m "feat(flow): per-step timeouts with sane defaults"
```

---

## Self-review checklist (Phase 2)

- [ ] Conditionals, return, retry, on_failure, match.expr → Tasks 3, 5
- [ ] CEL evaluator → Task 4
- [ ] Plugin host (subprocess) + discovery → Tasks 6, 7
- [ ] Built-in steps: verify.playable, remux, extract.subs, strip.tracks, move, copy, delete, shell → Tasks 8, 9
- [ ] Notifiers + notify step → Task 10
- [ ] Sonarr/Lidarr/generic webhook + dedup → Task 11
- [ ] Per-step timeouts → Task 12
- [ ] Sources table replaces bootstrap Radarr token → Task 11 (note: Phase 1 e2e test must be updated to insert a `sources` row before POSTing)
- [ ] No placeholders, every code block is paste-ready
- [ ] Type names consistent (`Node`, `Flow`, `SubprocessStep`, `Registry`)
