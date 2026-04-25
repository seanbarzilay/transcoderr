import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

export default function Sources() {
  const qc = useQueryClient();
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });
  const [kind, setKind] = useState("radarr");
  const [name, setName] = useState("");
  const [token, setToken] = useState("");
  const [config, setConfig] = useState("{}");
  const create = useMutation({
    mutationFn: () =>
      api.sources.create({
        kind,
        name,
        secret_token: token,
        config: JSON.parse(config || "{}"),
      }),
    onSuccess: () => {
      setName("");
      setToken("");
      setConfig("{}");
      qc.invalidateQueries({ queryKey: ["sources"] });
    },
  });
  const del = useMutation({
    mutationFn: (id: number) => api.sources.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sources"] }),
  });

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Sources</h2>
        </div>
      </div>

      <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
        <div className="label" style={{ marginBottom: 8 }}>
          Add source
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            <option>radarr</option>
            <option>sonarr</option>
            <option>lidarr</option>
            <option>webhook</option>
          </select>
          <input placeholder="name" value={name} onChange={(e) => setName(e.target.value)} />
          <input
            placeholder="bearer token"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
          <input
            placeholder="config json (e.g. {})"
            value={config}
            onChange={(e) => setConfig(e.target.value)}
            style={{ flex: 1, minWidth: 240 }}
          />
          <button onClick={() => create.mutate()} disabled={!name.trim() || !token.trim()}>
            Add
          </button>
        </div>
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 100 }}>Kind</th>
              <th style={{ width: 180 }}>Name</th>
              <th>Webhook URL</th>
              <th style={{ width: 110 }}></th>
            </tr>
          </thead>
          <tbody>
            {(sources.data ?? []).map((s: any) => (
              <tr key={s.id}>
                <td>
                  <span className="label">{s.kind}</span>
                </td>
                <td>{s.name}</td>
                <td className="mono dim">
                  {s.kind === "webhook" ? `/webhook/${s.name}` : `/webhook/${s.kind}`}
                </td>
                <td>
                  <button className="btn-danger" onClick={() => del.mutate(s.id)}>
                    Delete
                  </button>
                </td>
              </tr>
            ))}
            {(sources.data ?? []).length === 0 && !sources.isLoading && (
              <tr>
                <td colSpan={4} className="empty">
                  No sources configured.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
