import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api } from "../api/client";

export default function FlowsList() {
  const qc = useQueryClient();
  const flows = useQuery({ queryKey: ["flows"], queryFn: api.flows.list });
  const [showNew, setShowNew] = useState(false);
  const [name, setName] = useState("");
  const [yaml, setYaml] = useState("name: example\ntriggers:\n  - radarr: [downloaded]\nsteps:\n  - use: probe\n");
  const create = useMutation({
    mutationFn: () => api.flows.create({ name, yaml }),
    onSuccess: () => { setShowNew(false); qc.invalidateQueries({ queryKey: ["flows"] }); }
  });

  return (
    <div style={{ padding: 24 }}>
      <h2>Flows</h2>
      <button onClick={() => setShowNew(true)}>New flow</button>
      {showNew && (
        <div style={{ background: "rgba(255,255,255,0.05)", padding: 16, borderRadius: 8, marginTop: 12 }}>
          <input placeholder="name" value={name} onChange={e => setName(e.target.value)} style={{ marginBottom: 8, width: "100%" }} />
          <textarea value={yaml} onChange={e => setYaml(e.target.value)} rows={10} style={{ width: "100%", fontFamily: "monospace", fontSize: 12 }} />
          <div style={{ marginTop: 8 }}>
            <button onClick={() => create.mutate()}>Create</button>{" "}
            <button onClick={() => setShowNew(false)} style={{ background: "transparent", border: "1px solid rgba(255,255,255,0.2)" }}>Cancel</button>
          </div>
        </div>
      )}
      <table style={{ marginTop: 16 }}>
        <thead><tr><th>Name</th><th>Enabled</th><th>Version</th></tr></thead>
        <tbody>
          {(flows.data ?? []).map(f => (
            <tr key={f.id}>
              <td><Link to={`/flows/${f.id}`}>{f.name}</Link></td>
              <td>{f.enabled ? "yes" : "no"}</td>
              <td>{f.version}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
