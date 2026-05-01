export type FlowSummary = { id: number; name: string; enabled: boolean; version: number };
export type FlowDetail  = FlowSummary & { yaml_source: string; parsed_json: any };
export type RunRow      = { id: number; flow_id: number; status: string; created_at: number; finished_at?: number; file_path: string };
export type RunEvent    = { id: number; job_id: number; ts: number; step_id?: string; kind: string; payload?: any };
export type Source      = { id: number; kind: string; name: string; config?: Record<string, any>; secret_token?: string };
export type Notifier    = { id: number; name: string; kind: string; config: any };
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
  requires: any;
  schema: any;
  path: string;
  summary: string | null;
  min_transcoderr_version: string | null;
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
