export type FlowSummary = { id: number; name: string; enabled: boolean; version: number };
export type FlowDetail  = FlowSummary & { yaml_source: string; parsed_json: any };
export type RunRow      = { id: number; flow_id: number; status: string; created_at: number; finished_at?: number; file_path: string };
export type RunEvent    = { id: number; job_id: number; ts: number; step_id?: string; kind: string; payload?: any };
export type Source      = { id: number; kind: string; name: string };
export type Notifier    = { id: number; name: string; kind: string; config: any };
export type Plugin      = { id: number; name: string; version: string; kind: string; enabled: boolean; schema: any };
