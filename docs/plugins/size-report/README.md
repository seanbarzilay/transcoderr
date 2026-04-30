# size-report

Records the input and output size of every transcoded file plus the
compression ratio, so notify templates can render lines like:

```
✓ /mnt/movies/Foo (2024)/Foo.mkv — saved 38.4% (12433551104 → 7659011840)
```

It's a working example of a transcoderr subprocess plugin: ~120 lines of
stdlib Python, no dependencies, two step names from one entrypoint.

## Install

Copy the directory into your data dir and restart the server:

```bash
cp -r docs/plugins/size-report /var/lib/transcoderr/plugins/
docker restart transcoderr
```

The `bin/run` file must be executable (`chmod +x bin/run`). Then enable
**size-report** under the **Plugins** page in the web UI.

Requires `python3` on the host PATH. The shipped Docker images include it.

## Use it

Two steps need to bracket your transcode work — `size.report.before` runs
early to capture the original size, `size.report.after` runs after `output`
to record the result:

```yaml
steps:
  - use: probe
  - use: size.report.before
  - use: plan.init
  - use: plan.video.encode
    with: { codec: x265, crf: 19, preset: fast }
  - use: plan.execute
  - use: output
    with: { mode: replace }
  - use: size.report.after
  - use: notify
    with:
      channel: tg-main
      template: "✓ {{ file.path }} — saved {{ size_report.ratio_pct }}% ({{ size_report.before_bytes }} → {{ size_report.after_bytes }})"
```

After both steps run, `ctx.steps.size_report` looks like:

```json
{
  "before_bytes": 12433551104,
  "after_bytes":  7659011840,
  "saved_bytes":  4774539264,
  "ratio_pct":    38.4
}
```

A negative `ratio_pct` means the new file is *larger* than the original
(some flows do this on purpose — e.g. transcoding to a less efficient codec
for compatibility). The plugin doesn't clamp — that's a real signal you
probably want to see in your notifications.

## How it works

Two JSON-RPC lines on stdin (`init`, `execute`), event lines on stdout
(`progress`, `log`, `context_set`, `result`). Host side: see
[`crates/transcoderr/src/plugins/subprocess.rs`](../../../crates/transcoderr/src/plugins/subprocess.rs).

`size.report.before` writes `{before_bytes}` into `ctx.steps.size_report`
via a `context_set` event. `size.report.after` reads it back, stats the
final file (which is `ctx.file.path` after `output.replace` updates it),
and overwrites the entry with the full result.
