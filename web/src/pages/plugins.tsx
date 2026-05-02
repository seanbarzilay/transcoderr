import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { marked } from "marked";
import { api } from "../api/client";
import type { Plugin, PluginDetail, CatalogEntry } from "../types";

type Tab = "installed" | "browse" | "catalogs";

export default function Plugins() {
  const [tab, setTab] = useState<Tab>("installed");

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Plugins</h2>
        </div>
      </div>

      <div className="plugin-tabs" role="tablist">
        <button
          className={"plugin-tab" + (tab === "installed" ? " is-active" : "")}
          onClick={() => setTab("installed")}
          role="tab"
        >
          Installed
        </button>
        <button
          className={"plugin-tab" + (tab === "browse" ? " is-active" : "")}
          onClick={() => setTab("browse")}
          role="tab"
        >
          Browse
        </button>
        <button
          className={"plugin-tab" + (tab === "catalogs" ? " is-active" : "")}
          onClick={() => setTab("catalogs")}
          role="tab"
        >
          Catalogs
        </button>
      </div>

      {tab === "installed" && <Installed />}
      {tab === "browse" && <Browse />}
      {tab === "catalogs" && <Catalogs />}
    </div>
  );
}

function Installed() {
  const qc = useQueryClient();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const browse = useQuery({ queryKey: ["plugin-catalog-entries"], queryFn: api.plugins.browse });
  const [openId, setOpenId] = useState<number | null>(null);

  const uninstall = useMutation({
    mutationFn: (id: number) => api.plugins.uninstall(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["plugins"] }),
  });
  const install = useMutation({
    mutationFn: ({ catalogId, name }: { catalogId: number; name: string }) =>
      api.plugins.install(catalogId, name),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["plugins"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
    onError: (e: Error) => alert(`Install failed.\n\n${e.message}`),
  });

  /// Index by name for the "update available?" check.
  const catalogByName = new Map<string, CatalogEntry>();
  for (const e of browse.data?.entries ?? []) catalogByName.set(e.name, e);

  return (
    <div className="surface">
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th style={{ width: 110 }}>Version</th>
            <th style={{ width: 130 }}>Kind</th>
            <th>Provides</th>
            <th style={{ width: 220 }}></th>
          </tr>
        </thead>
        <tbody>
          {(plugins.data ?? []).map((p: Plugin) => {
            const open = openId === p.id;
            const cat = catalogByName.get(p.name);
            const updateAvailable =
              cat &&
              p.catalog_id === cat.catalog_id &&
              cat.version !== p.version;
            return (
              <PluginRows
                key={p.id}
                plugin={p}
                open={open}
                onToggle={() => setOpenId(s => (s === p.id ? null : p.id))}
                onUninstall={() => {
                  if (confirm(`Uninstall plugin "${p.name}"? This deletes its directory.`)) {
                    uninstall.mutate(p.id);
                  }
                }}
                onUpdate={
                  updateAvailable && cat
                    ? () => install.mutate({ catalogId: cat.catalog_id, name: cat.name })
                    : undefined
                }
                updateAvailable={!!updateAvailable}
              />
            );
          })}
          {(plugins.data ?? []).length === 0 && !plugins.isLoading && (
            <tr>
              <td colSpan={5} className="empty">
                No plugins discovered.
                <div className="hint">
                  Use the Browse tab to install one, or drop a directory into{" "}
                  <code>data/plugins/</code> and restart.
                </div>
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

interface RowProps {
  plugin: Plugin;
  open: boolean;
  onToggle: () => void;
  onUninstall: () => void;
  onUpdate?: () => void;
  updateAvailable: boolean;
}

function PluginRows({ plugin, open, onToggle, onUninstall, onUpdate, updateAvailable }: RowProps) {
  return (
    <>
      <tr className={"plugin-row" + (open ? " is-open" : "")} aria-expanded={open}>
        <td
          className="mono"
          onClick={onToggle}
          role="button"
          tabIndex={0}
          onKeyDown={e => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onToggle();
            }
          }}
        >
          <span className="plugin-row-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>{" "}
          {plugin.name}
        </td>
        <td className="dim tnum">
          {plugin.version}
          {updateAvailable && <span className="plugin-update-badge">update</span>}
        </td>
        <td>
          <span className="label">{plugin.kind}</span>
        </td>
        <td className="mono dim">
          {plugin.provides_steps.length === 0 ? "—" : plugin.provides_steps.join(", ")}
        </td>
        <td>
          {onUpdate && <button onClick={onUpdate}>Update</button>}{" "}
          <button className="btn-danger" onClick={onUninstall}>
            Uninstall
          </button>
        </td>
      </tr>
      {open && (
        <tr className="plugin-detail-row">
          <td colSpan={5}>
            <PluginDetailPanel id={plugin.id} />
          </td>
        </tr>
      )}
    </>
  );
}

function PluginDetailPanel({ id }: { id: number }) {
  const detail = useQuery({
    queryKey: ["plugin", id],
    queryFn: () => api.plugins.get(id),
    staleTime: 60_000,
  });

  if (detail.isLoading) {
    return <div className="muted">Loading…</div>;
  }
  if (detail.error || !detail.data) {
    return <div style={{ color: "var(--bad)" }}>Failed to load plugin detail.</div>;
  }
  return <PluginDetailBody detail={detail.data} />;
}

function PluginDetailBody({ detail }: { detail: PluginDetail }) {
  const requiresEmpty =
    !detail.requires ||
    detail.requires === null ||
    (typeof detail.requires === "object" &&
      !Array.isArray(detail.requires) &&
      Object.keys(detail.requires).length === 0);

  return (
    <div className="plugin-detail">
      <div className="plugin-detail-section">
        {detail.summary && (
          <div className="muted" style={{ marginBottom: 8 }}>
            {detail.summary}
          </div>
        )}
        <div className="plugin-detail-grid">
          <div className="label">Path</div>
          <code className="dim">{detail.path}</code>

          {detail.min_transcoderr_version && (
            <>
              <div className="label">Min version</div>
              <div>
                <span className="plugin-update-badge">
                  v{detail.min_transcoderr_version}+
                </span>
              </div>
            </>
          )}

          <div className="label">Provides steps</div>
          <div>
            {detail.provides_steps.length === 0 ? (
              <span className="muted">none</span>
            ) : (
              detail.provides_steps.map(s => (
                <code key={s} className="plugin-detail-step">
                  {s}
                </code>
              ))
            )}
          </div>

          {detail.capabilities.length > 0 && (
            <>
              <div className="label">Capabilities</div>
              <div>
                {detail.capabilities.map(c => (
                  <code key={c} className="plugin-detail-step">
                    {c}
                  </code>
                ))}
              </div>
            </>
          )}

          {detail.runtimes.length > 0 && (
            <>
              <div className="label">Runtimes</div>
              <div>
                {detail.runtimes.map(r => (
                  <code key={r} className="plugin-detail-step">
                    {r}
                  </code>
                ))}
              </div>
            </>
          )}

          {detail.deps && (
            <>
              <div className="label">Deps</div>
              <code className="dim" style={{ wordBreak: "break-all" }}>{detail.deps}</code>
            </>
          )}

          {!requiresEmpty && (
            <>
              <div className="label">Requires</div>
              <pre className="plugin-detail-pre">
                {JSON.stringify(detail.requires, null, 2)}
              </pre>
            </>
          )}
        </div>
      </div>

      <div className="plugin-detail-section">
        <div className="label" style={{ marginBottom: 8 }}>
          README
        </div>
        {detail.readme ? (
          <div
            className="plugin-detail-readme"
            // README content originates from a plugin directory the
            // operator placed on the server filesystem -- same trust
            // boundary as the plugin binary, which already runs arbitrary
            // code. Markdown is rendered with marked's defaults.
            dangerouslySetInnerHTML={{ __html: marked.parse(detail.readme) as string }}
          />
        ) : (
          <div className="muted">
            No README.md in the plugin directory. Operators have to read the
            manifest to know how to use it.
          </div>
        )}
      </div>
    </div>
  );
}

function Browse() {
  const qc = useQueryClient();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const browse = useQuery({ queryKey: ["plugin-catalog-entries"], queryFn: api.plugins.browse });

  const install = useMutation({
    mutationFn: ({ catalogId, name }: { catalogId: number; name: string }) =>
      api.plugins.install(catalogId, name),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["plugins"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
    onError: (e: Error) => alert(`Install failed.\n\n${e.message}`),
  });

  const installedNames = new Set((plugins.data ?? []).map(p => p.name));

  return (
    <div className="surface">
      {(browse.data?.errors ?? []).length > 0 && (
        <div className="catalog-fetch-banner">
          <strong>{browse.data!.errors.length} catalog(s) unreachable:</strong>
          <ul>
            {browse.data!.errors.map(e => (
              <li key={e.catalog_id}>
                <code>{e.catalog_name}</code> — {e.error}
              </li>
            ))}
          </ul>
        </div>
      )}
      <table>
        <thead>
          <tr>
            <th>Plugin</th>
            <th style={{ width: 110 }}>Version</th>
            <th style={{ width: 160 }}>From</th>
            <th>Provides</th>
            <th style={{ width: 140 }}></th>
          </tr>
        </thead>
        <tbody>
          {(browse.data?.entries ?? []).map((e: CatalogEntry) => {
            const installed = installedNames.has(e.name);
            return (
              <tr key={`${e.catalog_id}/${e.name}`}>
                <td className="mono">
                  {e.name}
                  <div className="muted" style={{ fontSize: 11 }}>{e.summary}</div>
                </td>
                <td className="dim tnum">{e.version}</td>
                <td><span className="label">{e.catalog_name}</span></td>
                <td className="mono dim">
                  {e.provides_steps.length === 0 ? "—" : e.provides_steps.join(", ")}
                  {e.runtimes && e.runtimes.length > 0 && (
                    <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                      runtimes: {e.runtimes.join(", ")}
                    </div>
                  )}
                  {e.deps && (
                    <div className="muted" style={{ fontSize: 11, marginTop: 2, wordBreak: "break-all" }}>
                      deps: <code>{e.deps}</code>
                    </div>
                  )}
                </td>
                <td>
                  {installed ? (
                    <span className="dim">Installed</span>
                  ) : e.missing_runtimes && e.missing_runtimes.length > 0 ? (
                    <div>
                      <button disabled title={`Missing runtime(s) on the server's PATH: ${e.missing_runtimes.join(", ")}`}>
                        Install
                      </button>
                      <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
                        Missing: {e.missing_runtimes.join(", ")}
                      </div>
                    </div>
                  ) : (
                    <button onClick={() => {
                      if (confirm(`Install "${e.name}"? This plugin runs as the transcoderr user.`)) {
                        install.mutate({ catalogId: e.catalog_id, name: e.name });
                      }
                    }}>Install</button>
                  )}
                </td>
              </tr>
            );
          })}
          {(browse.data?.entries ?? []).length === 0 && !browse.isLoading && (
            <tr>
              <td colSpan={5} className="empty">
                No plugins available from configured catalogs.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

function Catalogs() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["plugin-catalogs"], queryFn: api.pluginCatalogs.list });

  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [authHeader, setAuthHeader] = useState("");
  const [priority, setPriority] = useState("0");
  const [addError, setAddError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.pluginCatalogs.create({
      name: name.trim(),
      url: url.trim(),
      auth_header: authHeader.trim() || undefined,
      priority: Number.parseInt(priority, 10) || 0,
    }),
    onSuccess: () => {
      setName(""); setUrl(""); setAuthHeader(""); setPriority("0");
      setAddError(null);
      qc.invalidateQueries({ queryKey: ["plugin-catalogs"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
    onError: (e: any) => setAddError(e?.message ?? "create failed"),
  });
  const del = useMutation({
    mutationFn: (id: number) => api.pluginCatalogs.delete(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["plugin-catalogs"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
  });
  const refresh = useMutation({
    mutationFn: (id: number) => api.pluginCatalogs.refresh(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] }),
  });

  return (
    <>
      <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
        <div className="label" style={{ marginBottom: 8 }}>Add catalog</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <input placeholder="name" value={name} onChange={e => setName(e.target.value)} style={{ minWidth: 180 }} />
          <input placeholder="https://.../index.json" value={url} onChange={e => setUrl(e.target.value)} style={{ flex: 1, minWidth: 280 }} />
          <input type="password" placeholder="auth header (optional)" value={authHeader} onChange={e => setAuthHeader(e.target.value)} style={{ minWidth: 220 }} />
          <input type="number" placeholder="priority" value={priority} onChange={e => setPriority(e.target.value)} style={{ width: 100 }} />
          <button onClick={() => create.mutate()} disabled={create.isPending || !name.trim() || !url.trim()}>Add</button>
        </div>
        {addError && <div style={{ color: "var(--bad)", marginTop: 8, fontSize: 12 }}>{addError}</div>}
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>URL</th>
              <th style={{ width: 90 }}>Priority</th>
              <th style={{ width: 160 }}>Last fetched</th>
              <th style={{ width: 220 }}></th>
            </tr>
          </thead>
          <tbody>
            {(list.data ?? []).map(c => (
              <tr key={c.id}>
                <td className="mono">{c.name}</td>
                <td className="dim mono" style={{ fontSize: 11, wordBreak: "break-all" }}>{c.url}</td>
                <td className="tnum dim">{c.priority}</td>
                <td className="dim" style={{ fontSize: 11 }}>
                  {c.last_fetched_at
                    ? new Date(c.last_fetched_at * 1000).toLocaleString()
                    : "never"}
                  {c.last_error && (
                    <div style={{ color: "var(--bad)", marginTop: 2 }}>{c.last_error}</div>
                  )}
                </td>
                <td>
                  <button className="btn-ghost" onClick={() => refresh.mutate(c.id)}>Refresh</button>{" "}
                  <button className="btn-danger"
                    onClick={() => {
                      if (confirm(`Delete catalog "${c.name}"?`)) del.mutate(c.id);
                    }}>Delete</button>
                </td>
              </tr>
            ))}
            {(list.data ?? []).length === 0 && !list.isLoading && (
              <tr><td colSpan={5} className="empty">No catalogs configured.</td></tr>
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}
