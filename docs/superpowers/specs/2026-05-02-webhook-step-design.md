# Webhook Step — Design Spec

**Date:** 2026-05-02
**Status:** Draft, pending implementation plan
**Author:** Brainstorming session, 2026-05-02

## Goal

Let operators fire an arbitrary HTTP request from inside a flow without
configuring a notifier first. Use case: trigger a downstream system,
hit a custom webhook, ping a tracking endpoint, etc.

## Tech choices

- **Builtin step**, not a plugin. HTTP is a primitive — every flow
  eventually wants one. Lives at
  `crates/transcoderr/src/steps/builtin/webhook.rs`.
- **Inline config in flow YAML.** No coupling to the existing notifier
  rows. Operators can still use the `notify` step with a webhook
  notifier when they want shared config; this is for one-off /
  per-flow HTTP calls.
- **Templating everywhere** (`url`, `headers`, `body`) via the same
  engine `notify` uses. Variables: `file.*`, `flow.*`, `steps.*`,
  `env.*`, `failed.*`.
- **Hard-fail by default** on network error or non-2xx; one-keyword
  opt-out (`ignore_errors: true`).
- **No response data exposed** to subsequent steps in v1. Additive
  later if needed.

## YAML schema

```yaml
- use: webhook
  with:
    url: https://example.com/api/notify         # required, templated
    method: POST                                # default POST. GET/PUT/PATCH/DELETE accepted.
    headers:                                    # optional. Values templated; keys are not.
      Content-Type: application/json
      Authorization: "Bearer {{ env.MY_TOKEN }}"
      X-Source: transcoderr
    body: |                                     # optional, templated
      {"file": "{{ file.path }}", "size": {{ file.size }}}
    timeout_seconds: 30                         # default 30, max 300
    ignore_errors: false                        # default false
```

### Field details

- **`url`** (string, required): The full URL. Templated. Must parse as
  a valid URL after templating; otherwise the step fails before
  sending. Schemes restricted to `http` and `https`.
- **`method`** (string, default `POST`): One of `GET`, `POST`, `PUT`,
  `PATCH`, `DELETE`. Case-insensitive in YAML; normalized to uppercase
  internally. Not templated.
- **`headers`** (map of string → string, optional): Header name to
  value. Values are templated; names are passed verbatim. Multi-valued
  headers are not supported in v1 (use one entry per name).
- **`body`** (string, optional): Request body. Templated. If absent
  for `POST`/`PUT`/`PATCH`, sent with `Content-Length: 0`. Forbidden
  for `GET`/`DELETE` (configuration error at parse time).
- **`timeout_seconds`** (integer, default `30`): Total request
  timeout. Clamped to `[1, 300]`.
- **`ignore_errors`** (bool, default `false`): If `true`, network
  errors and non-2xx responses are logged as `tracing::warn!` and the
  step reports success. If `false`, both fail the step.

## Files

**New:**
- `crates/transcoderr/src/steps/builtin/webhook.rs` — step impl + the
  `WebhookConfig` deserialized from `with:`
- `docs/flows/webhook.yaml` — example flow that fires a webhook
  on completion (and another on failure via `on_failure`)
- `crates/transcoderr/tests/step_webhook.rs` — integration tests
  using wiremock

**Modified:**
- `crates/transcoderr/src/steps/builtin/mod.rs` — register the new
  step via `register_all`
- `README.md` — one-line mention in the flow-step list

## Implementation notes

### Step trait

Implements the existing `Step` trait. The `execute` method:

1. Deserialize `with:` into `WebhookConfig` (typed; serde rejects
   unknown fields).
2. Render `url`, each header value, and `body` through the template
   engine using the run context.
3. Validate post-render: URL parses, scheme is http/https, method is
   in the allowed set, body absent for GET/DELETE.
4. Build a `reqwest::Client` with the configured timeout (or reuse a
   shared client — see "HTTP client" below).
5. Send the request. Capture status + first 1024 bytes of response
   body for error reporting.
6. Branch:
   - Success (2xx): return `Ok(StepProgress::Done)`. No outputs.
   - Non-2xx: format error `webhook: {METHOD} {url} → {status}: {body}`.
     If `ignore_errors`, `tracing::warn!` and return Ok. Otherwise
     return Err.
   - Network error: format error `webhook: {METHOD} {url}: {err}`.
     Same branch on `ignore_errors`.

### HTTP client

Reuse the existing `reqwest::Client` already constructed for notifier
webhooks (see `notify::webhook::*`). If that client is hard to thread
through, build a fresh `reqwest::Client` per step — the cost is a
single TLS handshake's worth of state, negligible for a step that
takes ≥10ms to a remote host. Decide at implementation time; the
plan should call this out as an open question for the implementer.

### Error message truncation

Response bodies can be giant HTML error pages. Truncate to 1024 bytes
in the error message — operators can hit the URL directly to see the
full payload if they need it. The `tracing::warn!` and the step error
both use the truncated form.

### Deserialization

`WebhookConfig` uses `#[serde(deny_unknown_fields)]` so a typo in the
operator's YAML (e.g. `urls:` for `url:`) is caught at flow-parse
time, not silently ignored. Same pattern as the existing builtin
steps.

## Tests

### Unit

- `templates_url_headers_body`: a `WebhookConfig` rendered against a
  mock context substitutes `{{ file.path }}`, `{{ env.X }}`,
  `{{ steps.foo.bar }}` correctly in all three places.
- `body_forbidden_for_get`: a config with `method: GET, body: "x"`
  fails validation.
- `clamps_timeout`: `timeout_seconds: 1000` clamps to 300; `0` clamps
  to 1.
- `rejects_non_http_scheme`: `url: ftp://...` fails validation.

### Integration (wiremock)

- `success_2xx_step_ok`: server returns 200; step succeeds.
- `non_2xx_step_fails`: server returns 500; step error includes
  status + (truncated) body.
- `non_2xx_with_ignore_errors_step_ok`: server returns 500;
  `ignore_errors: true`; step succeeds; warning logged.
- `network_error_step_fails`: URL points at `127.0.0.1:1` (refused);
  step error mentions the connect failure.
- `network_error_with_ignore_errors_ok`: same setup with
  `ignore_errors: true`; step succeeds.
- `headers_and_body_round_trip`: wiremock asserts the request had the
  templated `Authorization` header and a JSON body matching the
  rendered template.

## Out of scope (v1)

- **Retries.** Operator can wrap with their own retry step or rely on
  the failure flow.
- **Response parsing / chaining.** No `steps.<id>.response.*`
  namespace. Add later if a real flow needs it.
- **Multipart / file uploads.** Specialized; not the common case.
- **OAuth flows / token refresh.** Operator handles via env var or
  a sidecar.
- **Multi-valued headers.** Single value per name is enough for the
  vast majority of webhook payloads.
- **TLS client certs / custom CA bundles.** Use the system trust store
  via reqwest's defaults.

## Risks

- **SSRF.** Operators can template a URL from any context field
  (`file.path`, etc.). Since flows already run as the transcoderr user
  with full filesystem and ffmpeg access, this is a no-op uplift in
  the threat model. Don't add a host allowlist in v1; revisit if
  multi-tenant deployment ever happens.
- **Secrets in YAML.** Templated `Authorization` headers can pull
  `{{ env.X }}`, which is the right place. Document the pattern in
  the example flow.
- **Operator footgun: forgot `ignore_errors`.** A flaky third-party
  webhook would fail every flow. Document the opt-out clearly in the
  example.

## Success criteria

- A flow can fire a templated POST against a wiremock server in an
  integration test, with status + body asserted.
- `ignore_errors: true` swallows non-2xx and network errors with a
  warning log, no flow failure.
- `cargo test -p transcoderr` green; the new step's tests are in the
  passing set.
- The example `docs/flows/webhook.yaml` parses, dry-runs, and is
  understandable to someone reading it cold.
