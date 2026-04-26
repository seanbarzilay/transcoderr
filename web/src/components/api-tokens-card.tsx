import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

export default function ApiTokensCard() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["api-tokens"], queryFn: api.auth.tokens.list });
  const [name, setName] = useState("");
  const [revealed, setRevealed] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (n: string) => api.auth.tokens.create(n),
    onSuccess: (resp) => {
      setRevealed(resp.token);
      setName("");
      qc.invalidateQueries({ queryKey: ["api-tokens"] });
    },
  });

  const remove = useMutation({
    mutationFn: (id: number) => api.auth.tokens.remove(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["api-tokens"] }),
  });

  return (
    <div className="surface" style={{ padding: 16, marginTop: 16 }}>
      <div className="page-header" style={{ marginBottom: 12 }}>
        <h3 style={{ margin: 0 }}>API tokens</h3>
      </div>

      <table style={{ width: "100%", marginBottom: 12 }}>
        <thead>
          <tr>
            <th style={{ textAlign: "left" }}>Name</th>
            <th style={{ textAlign: "left" }}>Prefix</th>
            <th style={{ textAlign: "left" }}>Created</th>
            <th style={{ textAlign: "left" }}>Last used</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((t) => (
            <tr key={t.id}>
              <td>{t.name}</td>
              <td className="mono dim">{t.prefix}…</td>
              <td className="dim tnum">{new Date(t.created_at * 1000).toLocaleString()}</td>
              <td className="dim tnum">
                {t.last_used_at ? new Date(t.last_used_at * 1000).toLocaleString() : "—"}
              </td>
              <td>
                <button
                  className="btn-danger"
                  onClick={() => {
                    if (confirm(`Revoke token "${t.name}"?`)) remove.mutate(t.id);
                  }}
                >
                  Revoke
                </button>
              </td>
            </tr>
          ))}
          {(list.data ?? []).length === 0 && !list.isLoading && (
            <tr><td colSpan={5} className="empty">No tokens.</td></tr>
          )}
        </tbody>
      </table>

      <div style={{ display: "flex", gap: 8 }}>
        <input
          placeholder="token name (e.g. claude-desktop)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          style={{ flex: 1 }}
        />
        <button
          onClick={() => create.mutate(name)}
          disabled={!name.trim() || create.isPending}
        >
          Create token
        </button>
      </div>

      {create.isError && (
        <div style={{ color: "#f88", fontSize: 12, marginTop: 6 }}>
          {(create.error as Error)?.message ?? "create failed"}
        </div>
      )}

      {revealed && (
        <div className="surface" style={{ padding: 12, marginTop: 12, borderColor: "var(--ok)" }}>
          <div className="label" style={{ marginBottom: 6 }}>
            New token — copy it now, this is the only time it will be shown
          </div>
          <code className="mono" style={{ wordBreak: "break-all" }}>{revealed}</code>
          <div style={{ marginTop: 8, display: "flex", gap: 8 }}>
            <button onClick={() => navigator.clipboard.writeText(revealed)}>Copy</button>
            <button className="btn-ghost" onClick={() => setRevealed(null)}>I've saved it</button>
          </div>
        </div>
      )}

      {remove.isError && (
        <div style={{ color: "#f88", fontSize: 12, marginTop: 6 }}>
          {(remove.error as Error)?.message ?? "revoke failed"}
        </div>
      )}
    </div>
  );
}
