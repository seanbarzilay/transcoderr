# Plugin catalog — design spec

**Date:** 2026-05-01
**Status:** approved (operator) — pending implementation plan
**Related:** [phase-2 plugin design](./2026-04-25-transcoderr-design.md), [size-report example plugin](../../plugins/size-report/)

## Goal

Operators install transcoderr plugins from a curated, browsable catalog
inside the web UI. The current path — copy a directory into
`{data_dir}/plugins/` and restart — keeps working as the manual fallback,
but the primary experience is "open Plugins → Browse → click Install."

Operators can also add their own catalogs (e.g. an internal-org repo of
private plugins) so the same UI surface drives both official and
custom-hosted plugins.

## Architecture

A catalog is one HTTP-reachable JSON file plus a set of plugin-directory
tarballs the index points at. Anyone can host one — there is no central
registry, no publish flow, no semver tagging. Transcoderr ships with one
default catalog row pointing at the official `transcoderr-plugins` repo
and stores any additional catalogs the operator adds in a new DB table.

```
┌────────────────┐        ┌────────────────┐
│ Plugins UI     │ HTTPS  │ Catalog        │
│ - Installed    │ ─────▶ │  index.json    │
│ - Browse       │        │  *.tar.gz      │
│ - Catalogs     │ ◀───── │                │
└────────────────┘        └────────────────┘
        │
        │ POST install
        ▼
┌────────────────┐        ┌────────────────────┐
│ Installer      │ ─────▶ │ {data_dir}/plugins │
│ - fetch tar    │        │   <name>/          │
│ - verify sha   │        │     manifest.toml  │
│ - extract      │        │     bin/run        │
│ - swap atom    │        │     ...            │
│ - re-discover  │        │                    │
│ - re-sync DB   │        │                    │
│ - swap reg.    │        │                    │
└────────────────┘        └────────────────────┘
```

Three new server modules:

- **Catalog client** — fetches each configured catalog's `index.json`,
  merges the entries, returns a unified list to the API. Failed fetches
  degrade gracefully (the failing catalog is omitted but the others still
  render, with a banner in the UI naming who failed and why).
- **Plugin installer** — given a catalog entry, downloads the tarball,
  verifies its sha256 and layout, atomically swaps it into
  `{data_dir}/plugins/<name>/`, re-runs `discover()` + `sync_discovered()`,
  and live-replaces the in-memory step registry so newly-provided steps
  dispatch without a server restart.
- **Plugin uninstaller** — `rm -rf` the directory, drop the DB row, swap
  the registry to drop the steps. Mirror of installer.

The official catalog is seeded by a one-row migration. The existing
in-tree manual-copy install path is unchanged for everything else.
`docs/plugins/size-report/` migrates out of this repo into the new
`transcoderr-plugins` repo as the seed entry; we delete it from this
repo at the same time so there's one canonical home for the example.

## Catalog schema

A catalog is one URL serving one JSON file. Per-plugin tarballs are
referenced by URL inside the index, so the catalog can host them
anywhere (GitHub raw, release assets, S3, gist, behind a CDN).

```json
{
  "schema_version": 1,
  "catalog_name": "transcoderr official",
  "catalog_url": "https://github.com/seanbarzilay/transcoderr-plugins",
  "plugins": [
    {
      "name": "size-report",
      "version": "0.1.0",
      "summary": "Records before/after byte counts and compression ratio.",
      "tarball_url": "https://github.com/.../size-report-0.1.0.tar.gz",
      "tarball_sha256": "abc123...",
      "homepage": "https://github.com/.../tree/main/size-report",
      "min_transcoderr_version": "0.18.0",
      "kind": "subprocess",
      "provides_steps": ["size.report.before", "size.report.after"]
    }
  ]
}
```

Three properties matter:

- **`tarball_sha256` is required.** The installer hashes what it
  downloads and bails if the hash doesn't match. This is the only
  thing standing between an operator and an MITM/typo'd-hostname
  scenario running arbitrary code on their server, so it is not
  optional. The catalog author commits to a specific tarball; the
  server enforces it.
- **`min_transcoderr_version`** lets a plugin author say "this needs
  the post-0.18 plugin DB sync." Server still surfaces the row but
  with a disabled Install button and a "Requires v0.X.0" badge so
  operators see what's coming when they upgrade.
- **`provides_steps`** is shown in the catalog list so operators can
  see what the plugin exposes before installing — same data they'd
  see in the post-install detail panel. Lets them decide "yes I want
  this" without expanding the row.

The plugin tarball itself is a directory tarball: at the top level a
single directory whose name matches the catalog entry's `name`,
containing `manifest.toml`, `bin/run`, optional `schema.json`,
optional `README.md`. Exactly the shape that already lives in
`{data_dir}/plugins/<name>/`. The installer rejects tarballs that
don't have this shape.

## Server components

### `crates/transcoderr/src/plugins/catalog.rs`

```rust
struct Catalog {
    id: i64,
    name: String,
    url: String,
    auth_header: Option<String>,  // e.g. "Bearer ..." for private catalogs
    priority: i32,                 // lower = wins on name conflicts
}

struct CatalogEntry {
    catalog_id: i64,
    catalog_name: String,
    name: String,
    version: String,
    summary: String,
    tarball_url: String,
    tarball_sha256: String,
    homepage: Option<String>,
    min_transcoderr_version: Option<String>,
    kind: String,
    provides_steps: Vec<String>,
}

impl CatalogClient {
    async fn fetch_index(catalog: &Catalog) -> anyhow::Result<Index>;
    async fn list_all(catalogs: &[Catalog]) -> Vec<CatalogEntryOrError>;
}
```

- `fetch_index` GETs `catalog.url`, attaches `Authorization: <auth_header>`
  if present, parses as JSON, validates `schema_version == 1`.
- `list_all` fetches all catalogs in parallel via `tokio::join_all`,
  caches results in-memory for 5 minutes (TTL keyed by catalog id),
  returns per-catalog success-or-error so the UI can surface failures
  individually.
- **Conflict resolution:** if two catalogs ship a plugin with the same
  `name`, both entries appear in the merged list, each tagged with its
  catalog-of-origin. Install is gated on the directory name, so only
  one can be installed at a time — picking which is the operator's
  choice.

### `crates/transcoderr/src/plugins/installer.rs`

```
install(catalog_entry, app_state):
  1. Stream tarball_url to a temp file. (reqwest, no full-buffer.)
  2. sha256 the file as we write; bail if mismatch.
  3. Untar into {data_dir}/plugins/.tcr-install.<rand>/  (staging).
  4. Verify staging contains exactly one top-level dir matching
     catalog_entry.name.
  5. Verify staging/<name>/manifest.toml parses and `name` matches.
  6. If {data_dir}/plugins/<name>/ exists -> atomic-rename to
     {data_dir}/plugins/.tcr-old.<name>.<rand>/.
     Then rename staging/<name>/ to {data_dir}/plugins/<name>/.
     Delete .tcr-old on success.
  7. Re-run plugins::discover(plugins_dir).
  8. Re-run db::plugins::sync_discovered(pool, &discovered).
  9. Swap the in-memory step registry (Arc<Registry> via RwLock,
     see below) so newly-provided steps dispatch without a restart.
```

Mid-way failure cleans up: `.tcr-install.*` and `.tcr-old.*` get `rm -rf`d.
The old plugin survives any failure of steps 4–6. Only step 6's atomic
rename actually mutates the visible plugins dir.

### Live registry replace

Currently `steps::registry::REGISTRY` is a `OnceCell<Arc<Registry>>` set
once at boot. Swap to:

```rust
static REGISTRY: OnceCell<RwLock<Arc<Registry>>> = OnceCell::const_new();

pub async fn resolve(name: &str) -> Option<Arc<dyn Step>> {
    REGISTRY.get()?.read().await.by_name.get(name).cloned()
}

/// Called by installer + uninstaller after they finish on-disk work.
pub async fn rebuild_from_discovered(...) {
    let new = build_registry(...);
    *REGISTRY.get().unwrap().write().await = Arc::new(new);
}
```

`resolve` returns `Arc<dyn Step>`; in-flight runs hold their pre-swap
`Arc` so they keep dispatching against the old code. New runs hit the
new registry. No tearing — `Arc` is the boundary.

### `crates/transcoderr/src/db/plugins.rs` extensions

Two new columns in the `plugins` table:

- `catalog_id INTEGER` — NULL means installed manually (the existing
  size-report-style flow). Non-NULL ties the row to the catalog it came
  from, used by the UI to detect "update available."
- `tarball_sha256 TEXT` — the sha of the tarball at install time. Used
  for the future "drifted from catalog?" check; not surfaced in v1
  but stored so the data is there when we need it.

## API surface (admin-authed)

| Method | Path                                                  | Purpose                            |
|--------|-------------------------------------------------------|------------------------------------|
| GET    | `/api/plugin-catalogs`                                | list configured catalogs           |
| POST   | `/api/plugin-catalogs`                                | add `{name, url, auth_header?}`    |
| DELETE | `/api/plugin-catalogs/:id`                            | remove                             |
| POST   | `/api/plugin-catalogs/:id/refresh`                    | bust the 5-min cache for one      |
| GET    | `/api/plugin-catalog-entries`                         | merged list across all catalogs    |
| POST   | `/api/plugin-catalog-entries/:catalog_id/:name/install` | invoke installer                  |
| DELETE | `/api/plugins/:id`                                    | uninstall (extends existing route) |

`auth_header` follows the same redact-and-unredact round-trip pattern
`api/auth.rs` already uses for notifier secrets: the value is replaced
with `***` in any list/get response served to a token-authed caller,
and on a PUT a `***` value is treated as "keep current" (the row's
existing `auth_header` is preserved). Cookie-authed responses see the
real value, same as notifiers do. The implementation adds a parallel
`SECRET_CATALOG_KEYS = &["auth_header"]` constant rather than reusing
the notifier list, since the schemas are different.

## UI changes

The Plugins page (`web/src/pages/plugins.tsx`) gets a tab strip:

```
Plugins
─────────────────────────────────────
  [Installed]  [Browse]  [Catalogs]
```

### Installed tab

What's there today: list of installed plugins, expand-on-click for the
manifest + README detail panel from #52. Two new actions per row:

- **Update** — visible when `installed.version != catalog.version` for
  the matching `(catalog_id, name)`. Calls the same install endpoint;
  the existing dir gets atomic-replaced.
- **Uninstall** — red, calls DELETE `/api/plugins/:id`. Confirm dialog.

### Browse tab

Table of all catalog entries from all configured catalogs:

```
Plugin               Version   From               Provides             
─────────────────────────────────────────────────────────────────────
size-report          0.1.0     official           size.report.before   [Install]
                                                  size.report.after
hash-write           0.2.0     official           hash.write           [Install]
my-internal          1.0.0     myorg              myorg.foo            [Installed]
```

Click a row → expand to show the plugin's full description + provides
list (same panel layout as Installed). **Install** triggers the install
action with a `confirm()` dialog ("This plugin will run as the
transcoderr user. Continue?"). On success, the row swaps to **Installed**
and the Installed tab gains the row.

A red banner at the top of the tab surfaces fetch failures:
"1 catalog unreachable: myorg-internal — last error: 503". Click to
expand and see all failures.

### Catalogs tab

Admin form for managing catalogs:

- Table of configured catalogs (name, url, priority, last-fetched-at,
  last-error?)
- Add row form: `name`, `url`, optional `auth_header` (password field).
- Per-row actions: Edit, Delete, Refresh.
- The official catalog row has Edit disabled but Delete enabled (so an
  operator running a private-catalog-only setup can opt out — niche but
  cheap to support).

State management: `useQuery(["plugin-catalog-entries"])` for the Browse
tab. Install/uninstall mutations invalidate both `["plugins"]` and
`["plugin-catalog-entries"]` so badges (Installed / Update available)
update without a manual refresh.

Visual style matches Notifiers / Sources — same surface backgrounds,
same table rules, same expand-on-click pattern from #52.

## DB schema

```sql
-- New table.
CREATE TABLE plugin_catalogs (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  url             TEXT NOT NULL,
  auth_header     TEXT,                       -- redacted to *** for token auth
  priority        INTEGER NOT NULL DEFAULT 0,
  last_fetched_at INTEGER,
  last_error      TEXT,
  created_at      INTEGER NOT NULL
);

-- Seed the official catalog. Operators can DELETE it if they want a
-- private-only setup.
INSERT INTO plugin_catalogs (name, url, priority, created_at)
VALUES ('transcoderr official',
        'https://raw.githubusercontent.com/seanbarzilay/transcoderr-plugins/main/index.json',
        0,
        strftime('%s', 'now'));

-- Extend existing plugins table.
ALTER TABLE plugins ADD COLUMN catalog_id      INTEGER;
ALTER TABLE plugins ADD COLUMN tarball_sha256  TEXT;
```

## Error handling

| Failure                          | Behavior                                                                                  |
|----------------------------------|-------------------------------------------------------------------------------------------|
| Catalog fetch fails              | non-fatal; that catalog omitted from merged list, last_error stored, banner in Browse tab |
| Tarball sha256 mismatch          | install bails 422 with reason; UI shows inline error; staging dir cleaned                  |
| Tarball layout wrong             | install bails 422 with specific reason ("expected single top-level dir 'X'")               |
| Manifest unparseable             | install bails 422; old plugin (if any) untouched (failure happens before atomic rename)    |
| `min_transcoderr_version` unmet  | row appears in Browse with disabled Install + "Requires v0.X.0" badge                      |
| Live-replace race (run mid-flight)| in-flight runs hold pre-swap Arc<dyn Step>, finish on old code; new runs see new registry |
| Catalog name conflict            | both entries appear in Browse tagged by catalog; only one can be installed at a time       |

## Testing

- **Catalog client** (wiremock): fetch success, 503 (graceful degrade),
  bad JSON, 401 with `auth_header`, schema_version drift, multiple
  catalogs in parallel where one fails.
- **Installer** (real tar.gz fixtures in `tests/fixtures/plugin_tarballs/`):
  happy path, sha mismatch, wrong top-level dir, malformed manifest,
  atomic-rename rollback when the new manifest is unparseable, install-
  over-existing replaces cleanly.
- **Live-replace registry**: register two plugins, run one, swap the
  registry mid-run via a barrier, assert the in-flight run completes
  successfully on the old code while a fresh `resolve` returns the
  new step.
- **API smoke test**: POST a catalog, GET the merged entry list (with
  one local mock catalog hosting one plugin via wiremock), POST install,
  GET `/api/plugins` and confirm the new row.
- **Integration: full install round-trip**: boot test app, mock a
  catalog serving the existing `size-report` tarball, install via the
  API, run a flow that uses `size.report.before` → `size.report.after`
  end-to-end, assert `ctx.steps.size_report` looks right after.

## Out of scope (deferred)

- **Custom URL install** (paste a tarball URL with no catalog backing).
  Operators can already do this by hosting their own catalog with one
  entry — same security surface, but at least the index points at the
  tarball with a known sha256.
- **Per-plugin semver pinning.** Catalog serves one current version per
  plugin name. Older versions only via the catalog repo's git history.
- **Auto-update.** "Update available" is a notification, not an action.
  Operators always click Update themselves.
- **Plugin signing / signature verification beyond sha256.** sha256 in
  the catalog covers tarball integrity; trust in the catalog itself is
  positional (the operator chose to add it). Cosign-style signing can
  layer on later if needed.

## Migration

- Add `plugin_catalogs` table + the `plugins.catalog_id` and
  `plugins.tarball_sha256` columns. Existing `plugins` rows get NULL
  for both — they're treated as manual installs and update detection
  is skipped for them.
- Move `docs/plugins/size-report/` from this repo to a new
  `seanbarzilay/transcoderr-plugins` repo as the seed entry. Delete it
  from this repo. The README's "Documentation" pointer to
  `docs/plugins/` becomes a pointer to the new repo.
