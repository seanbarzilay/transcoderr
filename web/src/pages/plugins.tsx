import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { marked } from "marked";
import { api } from "../api/client";
import type { Plugin, PluginDetail } from "../types";

export default function Plugins() {
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const [openId, setOpenId] = useState<number | null>(null);

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Plugins</h2>
        </div>
      </div>
      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th style={{ width: 110 }}>Version</th>
              <th style={{ width: 130 }}>Kind</th>
              <th>Provides</th>
            </tr>
          </thead>
          <tbody>
            {(plugins.data ?? []).map((p: Plugin) => (
              <PluginRows
                key={p.id}
                plugin={p}
                open={openId === p.id}
                onToggle={() => setOpenId(s => (s === p.id ? null : p.id))}
              />
            ))}
            {(plugins.data ?? []).length === 0 && !plugins.isLoading && (
              <tr>
                <td colSpan={4} className="empty">
                  No plugins discovered.
                  <div className="hint">
                    Drop a plugin directory into <code>data/plugins/</code> and restart.
                  </div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

interface RowProps {
  plugin: Plugin;
  open: boolean;
  onToggle: () => void;
}

function PluginRows({ plugin, open, onToggle }: RowProps) {
  return (
    <>
      <tr
        className={"plugin-row" + (open ? " is-open" : "")}
        onClick={onToggle}
        role="button"
        aria-expanded={open}
        tabIndex={0}
        onKeyDown={e => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <td className="mono">
          <span className="plugin-row-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>{" "}
          {plugin.name}
        </td>
        <td className="dim tnum">{plugin.version}</td>
        <td>
          <span className="label">{plugin.kind}</span>
        </td>
        <td className="mono dim">
          {plugin.provides_steps.length === 0
            ? "—"
            : plugin.provides_steps.join(", ")}
        </td>
      </tr>
      {open && (
        <tr className="plugin-detail-row">
          <td colSpan={4}>
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
        <div className="plugin-detail-grid">
          <div className="label">Path</div>
          <code className="dim">{detail.path}</code>

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
