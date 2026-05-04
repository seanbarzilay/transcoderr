# transcoderr → transcoderr relay

Chain two transcoderr instances together: one fires a `webhook` step
at the other's webhook source, and the second instance enqueues its
own job. The plumbing already exists — this doc is the contract.

Use cases:

- Redirect heavy encodes from a CPU-only node to a GPU box.
- Stage a transcode on a working instance, replicate the result to
  another instance for verification or further processing.
- Fan a single Radarr push out to multiple transcoderr instances.

## Receiver setup

On the **downstream** instance:

1. **Settings → Sources → Add.** Pick `kind = webhook`. Give it a
   name (e.g. `relay`). Generate a secret token and save it — you'll
   need it on the upstream side.
2. Leave `config json` blank. The default `path_expr =
   steps.payload.path` matches the body shape this doc recommends, so
   no extra config is needed.
3. **Flows.** Wire one or more flows on the downstream to fire on
   the new source.

## Sender setup

On the **upstream** instance:

1. Set the secret token on the container as `RELAY_TOKEN` (any name,
   referenced via `{{ env.RELAY_TOKEN }}` in the flow). Don't put the
   token in the YAML directly.
2. Add a `webhook` step at the end of the upstream flow:

   ```yaml
   - use: webhook
     with:
       url: https://downstream.example/webhook/relay
       headers:
         Authorization: "Bearer {{ env.RELAY_TOKEN }}"
       body: '{"path": "{{ file.path }}"}'
   ```

A complete example lives at
[`docs/flows/transcoderr-relay.yaml`](flows/transcoderr-relay.yaml).

## Path-mapping

The downstream instance must be able to read the file at the path it
was passed. Two cases:

- **Two coordinators with their own filesystems.** The sender (relay
  source) ships a path string; the receiver opens it. If the
  filesystems differ, set the receiver's `path_expr` to a CEL
  transformation that rewrites the path
  (e.g. `"steps.payload.path".replace("/upstream/", "/local/")`), or
  rewrite the path in the sender's `body` before sending.
- **Single coordinator with remote workers.** This is a different
  layer entirely. Per-worker path mappings (Workers page →
  **Edit mappings**) handle coordinator ↔ worker prefix translation
  on the dispatcher's wire path; you don't need a relay or CEL hook
  for that.

## Idempotency

The receiver dedups on `(source_id, path, raw_body)` for ~5 minutes
(existing dedup behavior). Re-firing the same body within that window
is safe and returns `202 Accepted` without enqueuing a duplicate job.
Varying the body (e.g. adding a timestamp) trips a second job.

## Failure handling

The `webhook` step hard-fails the upstream run on a non-2xx response
or network error. If the downstream is best-effort and you don't want
its outage to fail the upstream, set `ignore_errors: true`:

```yaml
- use: webhook
  with:
    url: https://downstream.example/webhook/relay
    headers:
      Authorization: "Bearer {{ env.RELAY_TOKEN }}"
    body: '{"path": "{{ file.path }}"}'
    ignore_errors: true
```

For an `on_failure:` chain, you can also relay the failure context
itself — see the example flow.

## Auth notes

Both Bearer (`Authorization: Bearer <token>`) and Basic auth (with the
token as the password) work — the receiver accepts either. Bearer is
recommended.

## Why no `kind=transcoderr` source kind?

The generic `kind=webhook` source already does everything needed:
named, authed, dedup'd, with a CEL path extractor. A typed kind would
just hardcode the `path_expr` default we already use here, for no real
operator benefit.
