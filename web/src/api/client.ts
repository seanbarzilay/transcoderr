const base = "/api";

async function req<T>(path: string, init: RequestInit = {}): Promise<T> {
  const r = await fetch(base + path, {
    credentials: "same-origin",
    headers: { "content-type": "application/json", ...init.headers },
    ...init,
  });
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}: ${await r.text()}`);
  if (r.status === 204) return undefined as T;
  return r.json();
}

export const api = {
  flows: {
    list:   () => req<import("../types").FlowSummary[]>("/flows"),
    get:    (id: number) => req<import("../types").FlowDetail>(`/flows/${id}`),
    create: (body: { name: string; yaml: string }) => req<{ id: number }>("/flows", { method: "POST", body: JSON.stringify(body) }),
    update: (id: number, body: { yaml: string; enabled?: boolean }) => req<void>(`/flows/${id}`, { method: "PUT", body: JSON.stringify(body) }),
    parse:  (yaml: string) => req<{ ok: boolean; error?: string; parsed?: any }>("/flows/parse", { method: "POST", body: JSON.stringify(yaml) }),
  },
  runs: {
    list:   (params?: { status?: string; flow_id?: number; limit?: number; offset?: number }) => {
      const q = new URLSearchParams(Object.entries(params ?? {}).filter(([,v]) => v != null).map(([k,v]) => [k, String(v)])).toString();
      return req<import("../types").RunRow[]>(`/runs${q ? `?${q}` : ""}`);
    },
    get:    (id: number) => req<{ run: import("../types").RunRow; events: import("../types").RunEvent[] }>(`/runs/${id}`),
    cancel: (id: number) => req<void>(`/runs/${id}/cancel`, { method: "POST" }),
    rerun:  (id: number) => req<{ id: number }>(`/runs/${id}/rerun`, { method: "POST" }),
  },
  sources: {
    list:   () => req<import("../types").Source[]>("/sources"),
    create: (body: any) => req<{id: number}>("/sources", { method: "POST", body: JSON.stringify(body) }),
    update: (id: number, body: any) => req<void>(`/sources/${id}`, { method: "PUT", body: JSON.stringify(body) }),
    delete: (id: number) => req<void>(`/sources/${id}`, { method: "DELETE" }),
  },
  plugins: {
    list:    () => req<import("../types").Plugin[]>("/plugins"),
    toggle:  (id: number, enabled: boolean) => req<void>(`/plugins/${id}`, { method: "PATCH", body: JSON.stringify({enabled}) }),
  },
  notifiers: {
    list:   () => req<import("../types").Notifier[]>("/notifiers"),
    create: (body: any) => req<{id: number}>("/notifiers", { method: "POST", body: JSON.stringify(body) }),
    update: (id: number, body: any) => req<void>(`/notifiers/${id}`, { method: "PUT", body: JSON.stringify(body) }),
    delete: (id: number) => req<void>(`/notifiers/${id}`, { method: "DELETE" }),
    test:   (id: number) => req<void>(`/notifiers/${id}/test`, { method: "POST" }),
  },
  settings: {
    get:   () => req<Record<string,string>>("/settings"),
    patch: (body: Record<string, any>) => req<void>("/settings", { method: "PATCH", body: JSON.stringify(body) }),
  },
  version: () => req<{ version: string }>("/version"),
  dryRun: (body: { yaml: string; file_path: string; probe?: any }) =>
    req<{ steps: any[]; probe: any }>("/dry-run", { method: "POST", body: JSON.stringify(body) }),
  auth: {
    me:     () => req<{ auth_required: boolean; authed: boolean }>("/auth/me"),
    login:  (password: string) => req<void>("/auth/login", { method: "POST", body: JSON.stringify({ password }) }),
    logout: () => req<void>("/auth/logout", { method: "POST" }),
    tokens: {
      list:   () => req<import("../types").ApiTokenSummary[]>("/auth/tokens"),
      create: (name: string) => req<{ id: number; token: string }>("/auth/tokens", { method: "POST", body: JSON.stringify({ name }) }),
      remove: (id: number) => req<void>(`/auth/tokens/${id}`, { method: "DELETE" }),
    },
  },
};
