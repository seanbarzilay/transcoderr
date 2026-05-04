import type { JsonObject } from "../types";

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
    parse:  (yaml: string) => req<{ ok: boolean; error?: string; parsed?: unknown }>("/flows/parse", { method: "POST", body: JSON.stringify(yaml) }),
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
    create: (body: JsonObject) => req<{id: number}>("/sources", { method: "POST", body: JSON.stringify(body) }),
    update: (id: number, body: JsonObject) => req<void>(`/sources/${id}`, { method: "PUT", body: JSON.stringify(body) }),
    delete: (id: number) => req<void>(`/sources/${id}`, { method: "DELETE" }),
  },
  plugins: {
    list:      () => req<import("../types").Plugin[]>("/plugins"),
    get:       (id: number) => req<import("../types").PluginDetail>(`/plugins/${id}`),
    uninstall: (id: number) => req<void>(`/plugins/${id}`, { method: "DELETE" }),
    browse:    () => req<import("../types").CatalogListResponse>("/plugin-catalog-entries"),
    install:   (catalogId: number, name: string) =>
      req<{ installed: string }>(`/plugin-catalog-entries/${catalogId}/${encodeURIComponent(name)}/install`, { method: "POST" }),
    /**
     * Open the install endpoint as an SSE stream. Returns an async iterator
     * over `{event, data}` records the caller can render live. The stream
     * ends when the server emits a terminal `done` or `error` event (the
     * server then closes the connection).
     *
     * Server emits:
     *  - status: { message }                — milestone log
     *  - log:    { stream: stdout|stderr, line }  — raw deps output
     *  - done:   { installed: <name> }      — terminal success
     *  - error:  { status, message }        — terminal failure
     */
    installStream: async function* (catalogId: number, name: string):
      AsyncGenerator<{ event: string; data: unknown }, void, unknown>
    {
      const url = `${base}/plugin-catalog-entries/${catalogId}/${encodeURIComponent(name)}/install`;
      const r = await fetch(url, { method: "POST", credentials: "same-origin" });
      if (!r.ok || !r.body) throw new Error(`${r.status} ${r.statusText}: ${await r.text()}`);
      const reader = r.body.getReader();
      const decoder = new TextDecoder();
      let buf = "";
      while (true) {
        const { value, done } = await reader.read();
        if (done) return;
        buf += decoder.decode(value, { stream: true });
        // SSE frames are separated by a blank line ("\n\n").
        let frameEnd: number;
        while ((frameEnd = buf.indexOf("\n\n")) !== -1) {
          const frame = buf.slice(0, frameEnd);
          buf = buf.slice(frameEnd + 2);
          let event = "message";
          const dataLines: string[] = [];
          for (const line of frame.split("\n")) {
            if (line.startsWith("event:")) event = line.slice(6).trim();
            else if (line.startsWith("data:")) dataLines.push(line.slice(5).trim());
          }
          if (dataLines.length === 0) continue;
          let data: unknown;
          try { data = JSON.parse(dataLines.join("\n")); } catch { data = dataLines.join("\n"); }
          yield { event, data };
        }
      }
    },
  },
  pluginCatalogs: {
    list:    () => req<import("../types").PluginCatalog[]>("/plugin-catalogs"),
    create:  (body: { name: string; url: string; auth_header?: string; priority?: number }) =>
      req<{ id: number }>("/plugin-catalogs", { method: "POST", body: JSON.stringify(body) }),
    delete:  (id: number) => req<void>(`/plugin-catalogs/${id}`, { method: "DELETE" }),
    refresh: (id: number) => req<void>(`/plugin-catalogs/${id}/refresh`, { method: "POST" }),
  },
  notifiers: {
    list:   () => req<import("../types").Notifier[]>("/notifiers"),
    create: (body: JsonObject) => req<{id: number}>("/notifiers", { method: "POST", body: JSON.stringify(body) }),
    update: (id: number, body: JsonObject) => req<void>(`/notifiers/${id}`, { method: "PUT", body: JSON.stringify(body) }),
    delete: (id: number) => req<void>(`/notifiers/${id}`, { method: "DELETE" }),
    test:   (id: number) => req<void>(`/notifiers/${id}/test`, { method: "POST" }),
  },
  settings: {
    get:   () => req<Record<string,string>>("/settings"),
    patch: (body: Record<string, unknown>) => req<void>("/settings", { method: "PATCH", body: JSON.stringify(body) }),
  },
  version: () => req<{ version: string }>("/version"),
  dryRun: (body: { yaml: string; file_path: string; probe?: unknown }) =>
    req<{ steps: Array<{ kind: string; use_or_label: string }>; probe: unknown }>("/dry-run", { method: "POST", body: JSON.stringify(body) }),
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
  workers: {
    list:   () => req<import("../types").Worker[]>("/workers"),
    create: (name: string) =>
      req<import("../types").WorkerCreateResp>("/workers", {
        method: "POST",
        body: JSON.stringify({ name }),
      }),
    patch: (id: number, body: { enabled: boolean }) =>
      req<import("../types").Worker>(`/workers/${id}`, {
        method: "PATCH",
        body: JSON.stringify(body),
      }),
    delete: (id: number) => req<void>(`/workers/${id}`, { method: "DELETE" }),
    updatePathMappings: (id: number, rules: Array<{ from: string; to: string }>) =>
      req<{ id: number; rules: Array<{ from: string; to: string }> }>(`/workers/${id}/path-mappings`, {
        method: "PUT",
        body: JSON.stringify({ rules }),
      }),
  },
  arr: {
    movies: (sourceId: number, params: import("../types-arr").BrowseParams) => {
      const q = new URLSearchParams(
        Object.entries(params).filter(([, v]) => v != null).map(([k, v]) => [k, String(v)])
      ).toString();
      return req<import("../types-arr").MoviesPage>(`/sources/${sourceId}/movies${q ? `?${q}` : ""}`);
    },
    series: (sourceId: number, params: import("../types-arr").BrowseParams) => {
      const q = new URLSearchParams(
        Object.entries(params).filter(([, v]) => v != null).map(([k, v]) => [k, String(v)])
      ).toString();
      return req<import("../types-arr").SeriesPage>(`/sources/${sourceId}/series${q ? `?${q}` : ""}`);
    },
    seriesGet: (sourceId: number, seriesId: number) =>
      req<import("../types-arr").SeriesDetail>(`/sources/${sourceId}/series/${seriesId}`),
    episodes: (sourceId: number, seriesId: number, params: import("../types-arr").EpisodesQuery = {}) => {
      const q = new URLSearchParams(
        Object.entries(params).filter(([, v]) => v != null && v !== "").map(([k, v]) => [k, String(v)])
      ).toString();
      return req<import("../types-arr").EpisodesPage>(`/sources/${sourceId}/series/${seriesId}/episodes${q ? `?${q}` : ""}`);
    },
    transcode: (sourceId: number, body: import("../types-arr").TranscodeReq) =>
      req<import("../types-arr").TranscodeResp>(`/sources/${sourceId}/transcode`, {
        method: "POST",
        body: JSON.stringify(body),
      }),
    refresh: (sourceId: number) =>
      req<void>(`/sources/${sourceId}/refresh`, { method: "POST" }),
  },
};
