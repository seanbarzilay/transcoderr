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
    mutationFn: () => api.sources.create({ kind, name, secret_token: token, config: JSON.parse(config || "{}") }),
    onSuccess: () => { setName(""); setToken(""); setConfig("{}"); qc.invalidateQueries({ queryKey: ["sources"] }); }
  });
  const del = useMutation({
    mutationFn: (id: number) => api.sources.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sources"] }),
  });

  return (
    <div style={{ padding: 24 }}>
      <h2>Sources</h2>
      <div style={{ display: "flex", gap: 8, marginBottom: 12, flexWrap: "wrap" }}>
        <select value={kind} onChange={e => setKind(e.target.value)}>
          <option>radarr</option><option>sonarr</option><option>lidarr</option><option>webhook</option>
        </select>
        <input placeholder="name" value={name} onChange={e => setName(e.target.value)} />
        <input placeholder="bearer token" value={token} onChange={e => setToken(e.target.value)} />
        <input placeholder='config json (e.g. {})' value={config} onChange={e => setConfig(e.target.value)} style={{ flex: 1 }} />
        <button onClick={() => create.mutate()}>Add</button>
      </div>
      <table>
        <thead><tr><th>Kind</th><th>Name</th><th>Webhook URL</th><th></th></tr></thead>
        <tbody>
          {(sources.data ?? []).map((s: any) => (
            <tr key={s.id}>
              <td>{s.kind}</td>
              <td>{s.name}</td>
              <td>{s.kind === "webhook" ? `/webhook/${s.name}` : `/webhook/${s.kind}`}</td>
              <td><button onClick={() => del.mutate(s.id)} style={{ background: "rgba(248,128,128,0.2)" }}>Delete</button></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
