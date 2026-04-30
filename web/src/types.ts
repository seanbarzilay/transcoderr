export type FlowSummary = { id: number; name: string; enabled: boolean; version: number };
export type FlowDetail  = FlowSummary & { yaml_source: string; parsed_json: any };
export type RunRow      = { id: number; flow_id: number; status: string; created_at: number; finished_at?: number; file_path: string };
export type RunEvent    = { id: number; job_id: number; ts: number; step_id?: string; kind: string; payload?: any };
export type Source      = { id: number; kind: string; name: string; config?: Record<string, any>; secret_token?: string };
export type Notifier    = { id: number; name: string; kind: string; config: any };
export type Plugin      = { id: number; name: string; version: string; kind: string; provides_steps: string[] };
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
  readme: string | null;
};
export type ApiTokenSummary = {
  id: number;
  name: string;
  prefix: string;
  created_at: number;
  last_used_at: number | null;
};
