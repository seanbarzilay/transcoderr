# Flow authoring reference

A flow is a YAML document describing what happens when a webhook fires.
Steps run sequentially. Conditional branches and templates use a small
subset of [CEL](https://github.com/google/cel-spec). This page documents
exactly what's available — what bindings are reachable, which CEL
functions and macros work, and the sharp edges that have actually
caught operators in the wild.

If you're authoring or editing a flow, run `POST /api/flows/validate`
(or the `validate_flow` MCP tool) on the YAML *before* saving. The
runtime evaluator silently treats CEL compile and execution errors in
`if:` as `false` — so a typo in a guard turns the branch off without
warning. The validator surfaces those errors statically.

## File shape

```yaml
name: my-flow                 # required, unique
description: |                # optional, free text
  ...
enabled: true                 # optional, defaults true
triggers:                     # required, at least one entry
  - radarr: [downloaded, upgraded]
  - sonarr: [downloaded]
  - lidarr: [downloaded]
  - webhook: my-source-name   # source kind=webhook, by name
match:                        # parsed but currently unused — see below
  expr: file.size_bytes > 1000
concurrency: 2                # optional, per-flow run cap
steps:                        # required, the body
  - ...
on_failure:                   # optional, runs if any step fails
  - ...
```

### Step nodes

Three node shapes. They share the `steps:` list and the `on_failure:`
list, and they nest freely.

**`Step`** — call a registered step (built-in or plugin):

```yaml
- id: my-step                 # optional, used in events + ctx.steps.<id>
  use: probe                  # required
  with:                       # optional, step-specific config
    timeout: 60
  retry:                      # optional
    max: 2
    on: "failed.error.contains('timeout')"  # optional CEL expr
  run_on: any                 # optional override; coordinator|any
```

**`Conditional`** — branch on a CEL bool:

```yaml
- id: gate                    # optional
  if: probe.streams != null
  then:
    - ...                     # any node sequence
  else:                       # optional
    - ...
```

**`Return`** — short-circuit out of the flow with a label:

```yaml
- return: skipped-no-streams
```

The label lands on the run record's status text. Anything after the
return in the current node sequence is skipped.

## Bindings reachable from CEL

Every `if:` and every `{{ ... }}` template inside a step's `with:` is
evaluated with these names in scope:

| Name | Shape | Notes |
|---|---|---|
| `file` | `{path: string, size_bytes: int?}` | The file the run is processing. `size_bytes` is `null` until `probe` runs (probe sets it from `metadata().len()`). |
| `probe` | `{streams: [...], format: {...}}` (ffprobe output) or `null` | Set by the `probe` step. Before `probe` runs, this is `null`. |
| `steps.<id>` | object | Output of any step that called `record_step_output(<id>, ...)`. Includes plugin context-set events. Look up by step `id`, or by the auto-generated id `<use>_<index>` when `id:` is omitted. |
| `failed` | `{id, use_, error}` or absent | Populated only inside an `on_failure:` block. Reference as `{{ failed.id }}` etc. |
| `env.<NAME>` | string | Process environment, useful for templates: `{{ env.MY_TOKEN }}`. Resolved per evaluation. |

There is **no** bare-key access to `steps.*` from templates — `{{
my_id.field }}` will fail with `UndeclaredReference`. Always go through
`steps.<id>`:

```yaml
template: "saved {{ steps.size_report.ratio_pct }}%"  # OK
template: "saved {{ size_report.ratio_pct }}%"         # WRONG
```

## Probe stream shape

The `probe` step shells out to `ffprobe -show_streams -show_format` and
puts the JSON straight on `ctx.probe`. The fields you'll touch most:

```jsonc
{
  "streams": [
    {
      "index": 0,
      "codec_type": "video",     // "video" | "audio" | "subtitle" | "data"
      "codec_name": "hevc",      // "h264" | "hevc" | "ac3" | "eac3" | "aac" | ...
      "channels": 6,             // audio only
      "color_transfer": "smpte2084",  // video only; HDR10 marker
      "tags": {                  // optional, keys vary
        "language": "eng",
        "title": "Director Commentary"
      },
      "disposition": {           // ffprobe disposition flags as 0|1
        "default": 1,
        "comment": 0,
        "attached_pic": 0,
        "forced": 0
      }
    }
  ],
  "format": {
    "duration": "5400.123",      // seconds, as a STRING — convert with double() if comparing
    "bit_rate": "8000000",
    "size": "5400000000",
    "tags": { ... }
  }
}
```

Stream fields **may be absent** rather than `null`. CEL distinguishes
"field not present" from "field is null"; see the `has()` rule below.

## CEL surface

The flow engine uses `cel-interpreter 0.10` with the `regex` and
`chrono` features enabled. The list below is exactly what's available
— missing entries (notably `lowerAscii` / `upperAscii`) will fail to
compile.

### Operators

`==` `!=` `<` `<=` `>` `>=` `+` `-` `*` `/` `%` `&&` `||` `!` `?:`
(ternary) and `in` (membership). Indexing with `[i]` works on lists and
`["k"]` on maps. `.field` on objects.

### Functions registered

| Function | Form | Notes |
|---|---|---|
| `contains` | `s.contains("x")` / `list.contains(x)` | Substring or list membership. |
| `startsWith` | `s.startsWith("x")` | |
| `endsWith` | `s.endsWith(".mkv")` | |
| `matches` | `s.matches("(?i)comment")` | Full Rust regex. Inline flags supported, e.g. `(?i)`. |
| `size` | `size(x)` | List/map/string length. |
| `min` `max` | `min(1, 2)` / `max(...)` | |
| `int` `string` `double` `uint` `bytes` | `int("42")` | Type coercion; useful when comparing `format.duration` (a string from ffprobe). |
| `duration` `timestamp` | chrono helpers | `getFullYear`, `getMonth`, etc. — see cel-interpreter docs if you need them. |

**Not registered** — calling these will raise a CEL compile error:
- `lowerAscii`, `upperAscii` — use `matches("(?i)...")` for
  case-insensitive comparison instead.

### Macros

| Macro | Form | Returns |
|---|---|---|
| `has` | `has(x.y.z)` | `true` if every parent is present AND `z` is present. See sharp edge below. |
| `exists` | `list.exists(x, predicate)` | `true` if predicate holds for at least one element. |
| `all` | `list.all(x, predicate)` | `true` if predicate holds for every element. |
| `filter` | `list.filter(x, predicate)` | Sub-list. |
| `map` | `list.map(x, expr)` | Transformed list. |

### Sharp edge — `has()` only gates the leaf

`has(x.y)` returns `true` if `y` exists on `x`, or errors if `x` itself
is missing. **It does not propagate down to `x.y.z`** — accessing
`x.y.z` afterward still throws if `z` isn't there.

The right pattern when checking a deeply nested optional field:

```cel
// WRONG — disposition is {} (present but empty), so has() returns true,
// then s.disposition.comment throws "no such key", and the whole `if:`
// silently evaluates to false.
has(s.disposition) && s.disposition.comment == 1

// RIGHT — has() the leaf you're about to access.
has(s.disposition.comment) && s.disposition.comment == 1
```

This is the single most common reason a flow guard "doesn't fire when
it should." Always `has()` the field you're about to dereference, not
its parent.

### Sharp edge — `if:` swallows compile/exec errors

The runtime `if:` evaluator does `Result<bool>::unwrap_or(false)`. A
typo or bad type access doesn't fail the run — it silently makes the
branch never fire. Catch these at authoring time:

- `POST /api/flows/validate {"yaml": "..."}` — returns `{ok, issues}`
  with one entry per CEL compile error.
- `validate_flow` MCP tool — same payload.

## Templates

Strings inside `with:` may contain `{{ expr }}` placeholders. Same CEL
surface as `if:`, plus all bindings above. Examples:

```yaml
- use: notify
  with:
    template: "✓ {{ file.path }} — saved {{ steps.size_report.ratio_pct }}%"

- use: webhook
  with:
    url: http://localhost:8080/webhook/relay
    headers:
      Authorization: "Bearer {{ env.RELAY_TOKEN }}"
    body: '{"path": "{{ file.path }}"}'
```

The template walker recurses through `with:`, including nested objects
and arrays — anything stringy with `{{` in it gets evaluated.

## Conventions

These aren't enforced by the engine, but built-in steps follow them and
your `if:` guards probably should too.

### Commentary detection

`plan.audio.ensure` considers an audio track "commentary" — and skips
it as a candidate for the wanted-codec check — when:

```cel
(has(s.disposition.comment) && s.disposition.comment == 1)
||
(has(s.tags.title) && s.tags.title.matches("(?i)comment"))
```

If you're writing a flow gate that should ignore commentary, mirror
that exactly.

### HDR detection

`plan.video.encode` reads the first video stream's `color_transfer`:
`smpte2084` → HDR10, `arib-std-b67` → HLG. Anything else is treated as
SDR. Dolby Vision falls back to the HDR10 base layer.

## Dead-but-still-parsed syntax

These appear in older flow examples; the parser accepts them, the
runtime ignores them. Treat as no-ops:

- **`match: { expr: ... }`** — parsed into `Flow.match_block`, but
  `match_expr()` has no caller. Do filtering inside `steps:` with a
  `Conditional` + `Return` instead:
  ```yaml
  steps:
    - if: file.size_bytes > 1000
      then: []
      else:
        - return: skipped-too-small
  ```
- **`file.size_gb`** — not bound. Use `file.size_bytes` (an integer)
  and convert: `file.size_bytes > 1000000000` for "≥1GB".

## Validation tooling

```bash
# Static check (recommended before saving):
curl -X POST http://localhost:8080/api/flows/validate \
  -H "Authorization: Bearer $T" \
  -H "Content-Type: application/json" \
  -d '{"yaml": "name: t\nenabled: true\ntriggers:\n  - radarr: [downloaded]\nsteps:\n  - use: noop\n"}'
# → {"ok": true, "issues": []}

# Or via MCP:
#   validate_flow(yaml="...")

# Dry-run against a synthetic file path + probe (walks the AST and
# resolves every conditional, but doesn't execute steps):
curl -X POST http://localhost:8080/api/dry-run \
  -H "Authorization: Bearer $T" \
  -H "Content-Type: application/json" \
  -d '{"yaml": "...", "file_path": "/tv/X.mkv", "probe": { ... }}'
# → {"steps": [{kind: "if-true"|"if-false"|"step"|"return", ...}]}

# Validation gives you compile errors. Dry-run shows you which branches
# would actually run for a given probe. Use both.
```
