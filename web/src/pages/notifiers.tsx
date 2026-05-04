import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import {
  KINDS,
  fromConfig,
  toConfig,
  validate,
} from "../lib/notifier-form";
import type { Kind, FormValue } from "../lib/notifier-form";
import NotifierForm from "../components/notifier-form";
import AddNotifierForm from "../components/forms/add-notifier";
import { errorMessage } from "../lib/errors";
import { asRecord } from "../lib/records";
import type { JsonObject, Notifier } from "../types";

export default function Notifiers() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["notifiers"], queryFn: api.notifiers.list });

  // Inline edit state, keyed by id
  const [editing, setEditing] = useState<Record<number, { name: string; value: FormValue }>>({});
  const [rowError, setRowError] = useState<Record<number, string | null>>({});

  const update = useMutation({
    mutationFn: ({ id, body }: { id: number; body: JsonObject }) =>
      api.notifiers.update(id, body),
    onSuccess: (_d, vars) => {
      setEditing(s => {
        const next = { ...s };
        delete next[vars.id];
        return next;
      });
      setRowError(s => ({ ...s, [vars.id]: null }));
      qc.invalidateQueries({ queryKey: ["notifiers"] });
    },
    onError: (e: unknown, vars) =>
      setRowError(s => ({ ...s, [vars.id]: errorMessage(e, "save failed") })),
  });

  const del = useMutation({
    mutationFn: (id: number) => api.notifiers.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["notifiers"] }),
  });

  const test = useMutation({
    mutationFn: (id: number) => api.notifiers.test(id),
    onSuccess: (_d, id) =>
      setRowError(s => ({ ...s, [id]: "✓ test sent" })),
    onError: (e: unknown, id) =>
      setRowError(s => ({ ...s, [id]: errorMessage(e, "test failed") })),
  });

  function startEdit(n: Notifier) {
    const k = (KINDS as readonly string[]).includes(n.kind) ? (n.kind as Kind) : null;
    if (!k) {
      setRowError(s => ({ ...s, [n.id]: `unknown kind "${n.kind}"` }));
      return;
    }
    setEditing(s => ({
      ...s,
      [n.id]: { name: n.name, value: fromConfig(k, n.config) },
    }));
    setRowError(s => ({ ...s, [n.id]: null }));
  }

  function cancelEdit(id: number) {
    setEditing(s => {
      const next = { ...s };
      delete next[id];
      return next;
    });
    setRowError(s => ({ ...s, [id]: null }));
  }

  function saveEdit(n: Notifier) {
    const draft = editing[n.id];
    if (!draft) return;
    const k = n.kind as Kind;
    const err = validate(k, draft.value, true);
    if (err) {
      setRowError(s => ({ ...s, [n.id]: err }));
      return;
    }
    update.mutate({
      id: n.id,
      body: { name: draft.name, kind: n.kind, config: toConfig(k, draft.value, true) },
    });
  }

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Notifiers</h2>
        </div>
      </div>

      <AddNotifierForm />

      <div className="surface">
      <table>
        <thead>
          <tr>
            <th style={{ width: 100 }}>Kind</th>
            <th style={{ width: 220 }}>Name</th>
            <th>Config</th>
            <th style={{ width: 240 }}></th>
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((n: Notifier) => {
            const draft = editing[n.id];
            const err = rowError[n.id];
            return (
              <tr key={n.id} style={{ verticalAlign: "top" }}>
                <td><span className="label">{n.kind}</span></td>
                <td>
                  {draft ? (
                    <input
                      value={draft.name}
                      onChange={e =>
                        setEditing(s => ({
                          ...s,
                          [n.id]: { ...s[n.id], name: e.target.value },
                        }))
                      }
                      style={{ width: "100%" }}
                    />
                  ) : (
                    n.name
                  )}
                </td>
                <td>
                  {draft ? (
                    <NotifierForm
                      kind={n.kind as Kind}
                      value={draft.value}
                      onChange={v =>
                        setEditing(s => ({
                          ...s,
                          [n.id]: { ...s[n.id], value: v },
                        }))
                      }
                      isEdit={true}
                    />
                  ) : (
                    <ConfigSummary config={n.config} />
                  )}
                  {err && (
                    <div
                      style={{
                        color: err.startsWith("✓") ? "var(--ok)" : "var(--bad)",
                        fontSize: 12,
                        marginTop: 4,
                      }}
                    >
                      {err}
                    </div>
                  )}
                </td>
                <td>
                  {draft ? (
                    <>
                      <button
                        onClick={() => saveEdit(n)}
                        disabled={update.isPending}
                      >
                        Save
                      </button>{" "}
                      <button onClick={() => cancelEdit(n.id)} className="btn-ghost">
                        Cancel
                      </button>
                    </>
                  ) : (
                    <>
                      <button className="btn-ghost" onClick={() => startEdit(n)}>
                        Edit
                      </button>{" "}
                      <button
                        className="btn-ghost"
                        onClick={() => test.mutate(n.id)}
                        disabled={test.isPending}
                      >
                        Test
                      </button>{" "}
                      <button
                        className="btn-danger"
                        onClick={() => {
                          if (confirm(`Delete notifier "${n.name}"?`)) del.mutate(n.id);
                        }}
                      >
                        Delete
                      </button>
                    </>
                  )}
                </td>
              </tr>
            );
          })}
          {(list.data ?? []).length === 0 && !list.isLoading && (
            <tr>
              <td colSpan={4} className="empty">
                No notifiers yet.
                <div className="hint">Add one above to get notifications on flow events.</div>
              </td>
            </tr>
          )}
        </tbody>
      </table>
      </div>
    </div>
  );
}

/// Read-only render of a notifier's config in the table — labels +
/// values, no JSON braces. Secrets are already redacted to `***` by
/// the server for token-authed callers.
function ConfigSummary({ config }: { config: unknown }) {
  if (!config || typeof config !== "object") {
    return <span className="muted">—</span>;
  }
  const entries = Object.entries(config);
  if (entries.length === 0) return <span className="muted">—</span>;
  return (
    <div className="notifier-config-summary">
      {entries.map(([k, v]) => (
        <div key={k} className="notifier-config-summary-row">
          <span className="notifier-config-summary-key">{k}</span>
          <span className="notifier-config-summary-val">
            {Array.isArray(v) ? <PathMappingsSummary value={v} /> : String(v)}
          </span>
        </div>
      ))}
    </div>
  );
}

function PathMappingsSummary({ value }: { value: unknown[] }) {
  if (value.length === 0) return <span className="muted">none</span>;
  return (
    <span>
      {value.map((m, i) => (
        <span key={i} className="notifier-config-mapping">
          {String(asRecord(m).from ?? "?")} → {String(asRecord(m).to ?? "?")}
          {i < value.length - 1 ? ", " : ""}
        </span>
      ))}
    </span>
  );
}
