import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

const KINDS = ["discord", "ntfy", "telegram", "webhook"] as const;

const PLACEHOLDERS: Record<string, string> = {
  discord:  '{"url":"https://discord.com/api/webhooks/..."}',
  ntfy:     '{"server":"https://ntfy.sh","topic":"my-topic"}',
  telegram: '{"bot_token":"123:ABC","chat_id":"-1001234"}',
  webhook:  '{"url":"https://example.com/hook"}',
};

type Notifier = { id: number; name: string; kind: string; config: any };

export default function Notifiers() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["notifiers"], queryFn: api.notifiers.list });

  // Add form
  const [kind, setKind] = useState<string>("discord");
  const [name, setName] = useState("");
  const [config, setConfig] = useState("{}");
  const [addError, setAddError] = useState<string | null>(null);

  // Inline edit state, keyed by id
  const [editing, setEditing] = useState<Record<number, { name: string; config: string }>>({});
  const [rowError, setRowError] = useState<Record<number, string | null>>({});

  const create = useMutation({
    mutationFn: () => {
      const body = { name, kind, config: JSON.parse(config || "{}") };
      return api.notifiers.create(body);
    },
    onSuccess: () => {
      setName("");
      setConfig("{}");
      setAddError(null);
      qc.invalidateQueries({ queryKey: ["notifiers"] });
    },
    onError: (e: any) => setAddError(e?.message ?? "create failed"),
  });

  const update = useMutation({
    mutationFn: ({ id, body }: { id: number; body: any }) =>
      api.notifiers.update(id, body),
    onSuccess: (_d, vars) => {
      setEditing(s => {
        const { [vars.id]: _gone, ...rest } = s;
        return rest;
      });
      setRowError(s => ({ ...s, [vars.id]: null }));
      qc.invalidateQueries({ queryKey: ["notifiers"] });
    },
    onError: (e: any, vars) =>
      setRowError(s => ({ ...s, [vars.id]: e?.message ?? "save failed" })),
  });

  const del = useMutation({
    mutationFn: (id: number) => api.notifiers.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["notifiers"] }),
  });

  const test = useMutation({
    mutationFn: (id: number) => api.notifiers.test(id),
    onSuccess: (_d, id) =>
      setRowError(s => ({ ...s, [id]: "✓ test sent" })),
    onError: (e: any, id) =>
      setRowError(s => ({ ...s, [id]: e?.message ?? "test failed" })),
  });

  function startEdit(n: Notifier) {
    setEditing(s => ({
      ...s,
      [n.id]: { name: n.name, config: JSON.stringify(n.config, null, 2) },
    }));
    setRowError(s => ({ ...s, [n.id]: null }));
  }

  function cancelEdit(id: number) {
    setEditing(s => {
      const { [id]: _gone, ...rest } = s;
      return rest;
    });
    setRowError(s => ({ ...s, [id]: null }));
  }

  function saveEdit(n: Notifier) {
    const draft = editing[n.id];
    if (!draft) return;
    let parsed: any;
    try {
      parsed = JSON.parse(draft.config || "{}");
    } catch (e: any) {
      setRowError(s => ({ ...s, [n.id]: `invalid JSON: ${e.message}` }));
      return;
    }
    update.mutate({
      id: n.id,
      body: { name: draft.name, kind: n.kind, config: parsed },
    });
  }

  return (
    <div style={{ padding: 24 }}>
      <h2>Notifiers</h2>

      <div
        style={{
          background: "rgba(255,255,255,0.05)",
          padding: 12,
          borderRadius: 6,
          marginBottom: 16,
        }}
      >
        <div style={{ fontSize: 12, opacity: 0.7, marginBottom: 8 }}>Add notifier</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <select
            value={kind}
            onChange={e => {
              setKind(e.target.value);
              if (config === "{}" || Object.values(PLACEHOLDERS).includes(config)) {
                setConfig(PLACEHOLDERS[e.target.value] ?? "{}");
              }
            }}
          >
            {KINDS.map(k => (
              <option key={k}>{k}</option>
            ))}
          </select>
          <input
            placeholder="name (referenced from flow YAML)"
            value={name}
            onChange={e => setName(e.target.value)}
            style={{ minWidth: 220 }}
          />
          <input
            placeholder={PLACEHOLDERS[kind] ?? '{"key":"value"}'}
            value={config}
            onChange={e => setConfig(e.target.value)}
            style={{ flex: 1, minWidth: 320, fontFamily: "monospace", fontSize: 12 }}
          />
          <button
            onClick={() => create.mutate()}
            disabled={create.isPending || !name.trim()}
          >
            Add
          </button>
        </div>
        {addError && (
          <div style={{ color: "#f88", marginTop: 8, fontSize: 12 }}>{addError}</div>
        )}
      </div>

      <table>
        <thead>
          <tr>
            <th style={{ width: 90 }}>Kind</th>
            <th style={{ width: 220 }}>Name</th>
            <th>Config</th>
            <th style={{ width: 220 }}></th>
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((n: Notifier) => {
            const draft = editing[n.id];
            const err = rowError[n.id];
            return (
              <tr key={n.id} style={{ verticalAlign: "top" }}>
                <td><code>{n.kind}</code></td>
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
                    <textarea
                      value={draft.config}
                      onChange={e =>
                        setEditing(s => ({
                          ...s,
                          [n.id]: { ...s[n.id], config: e.target.value },
                        }))
                      }
                      rows={4}
                      style={{ width: "100%", fontFamily: "monospace", fontSize: 12 }}
                    />
                  ) : (
                    <code style={{ opacity: 0.8, fontSize: 12 }}>
                      {JSON.stringify(n.config)}
                    </code>
                  )}
                  {err && (
                    <div
                      style={{
                        color: err.startsWith("✓") ? "#8f8" : "#f88",
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
                      <button
                        onClick={() => cancelEdit(n.id)}
                        style={{
                          background: "transparent",
                          border: "1px solid rgba(255,255,255,0.2)",
                        }}
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <>
                      <button onClick={() => startEdit(n)}>Edit</button>{" "}
                      <button
                        onClick={() => test.mutate(n.id)}
                        disabled={test.isPending}
                        style={{
                          background: "rgba(72,187,120,0.2)",
                        }}
                      >
                        Test
                      </button>{" "}
                      <button
                        onClick={() => {
                          if (confirm(`Delete notifier "${n.name}"?`)) del.mutate(n.id);
                        }}
                        style={{ background: "rgba(248,128,128,0.2)" }}
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
              <td colSpan={4} style={{ opacity: 0.6, padding: 16 }}>
                No notifiers yet. Add one above.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
