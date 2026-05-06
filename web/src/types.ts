export type JsonObject = Record<string, unknown>;

export type FlowSummary = { id: number; name: string; enabled: boolean; version: number };
export type FlowDetail  = FlowSummary & { yaml_source: string; parsed_json: unknown };
export type FlowValidationIssueKind = "yaml_parse_error" | "condition_compile_error" | "template_compile_error";
export type FlowValidationIssue = { path: string; kind: FlowValidationIssueKind; message: string };
export type FlowValidationReport = { ok: boolean; issues: FlowValidationIssue[] };
export type RunRow      = { id: number; flow_id: number; status: string; created_at: number; finished_at?: number; file_path: string };
export type RunEvent    = { id: number; job_id: number; ts: number; step_id?: string; kind: string; payload?: unknown; worker_id?: number; worker_name?: string };
export type Source      = { id: number; kind: string; name: string; config?: JsonObject; secret_token?: string };
export type Notifier    = { id: number; name: string; kind: string; config: JsonObject };
export type Plugin = {
  id: number;
  name: string;
  version: string;
  kind: string;
  provides_steps: string[];
  catalog_id?: number | null;
  tarball_sha256?: string | null;
};

export type PluginDetail = {
  id: number;
  name: string;
  version: string;
  kind: string;
  provides_steps: string[];
  capabilities: string[];
  requires: unknown;
  schema: unknown;
  path: string;
  summary: string | null;
  min_transcoderr_version: string | null;
  runtimes: string[];
  /// Shell command run on install + every boot (e.g.
  /// `pip install -r requirements.txt`).
  deps: string | null;
  readme: string | null;
};

export type PluginCatalog = {
  id: number;
  name: string;
  url: string;
  auth_header: string | null;
  priority: number;
  last_fetched_at: number | null;
  last_error: string | null;
};

export type CatalogEntry = {
  catalog_id: number;
  catalog_name: string;
  name: string;
  version: string;
  summary: string;
  tarball_url: string;
  tarball_sha256: string;
  homepage: string | null;
  min_transcoderr_version: string | null;
  kind: string;
  provides_steps: string[];
  /// Bare executable names the plugin needs on PATH.
  runtimes: string[];
  /// Subset of `runtimes` not on the server's PATH. Empty = installable.
  missing_runtimes: string[];
  /// Shell command run on install + every boot.
  deps: string | null;
};

export type CatalogFetchError = {
  catalog_id: number;
  catalog_name: string;
  error: string;
};

export type CatalogListResponse = {
  entries: CatalogEntry[];
  errors: CatalogFetchError[];
};
export type ApiTokenSummary = {
  id: number;
  name: string;
  prefix: string;
  created_at: number;
  last_used_at: number | null;
};

export type Worker = {
  id: number;
  name: string;
  kind: "local" | "remote";
  secret_token: string | null;       // "***" or null after mint
  hw_caps: unknown | null;
  plugin_manifest: unknown[] | null;
  enabled: boolean;
  last_seen_at: number | null;
  created_at: number;
  path_mappings: Array<{ from: string; to: string }> | null;
};

export type WorkerCreateResp = {
  id: number;
  secret_token: string;              // one-time-display
};
