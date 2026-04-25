import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api } from "../api/client";

const STARTER_YAML = `name: example
triggers:
  - radarr: [downloaded]
steps:
  - use: probe
  - use: plan.init
  - use: plan.execute
  - use: output
    with: { mode: replace }
`;

export default function FlowsList() {
  const qc = useQueryClient();
  const flows = useQuery({ queryKey: ["flows"], queryFn: api.flows.list });
  const [showNew, setShowNew] = useState(false);
  const [name, setName] = useState("");
  const [yaml, setYaml] = useState(STARTER_YAML);
  const create = useMutation({
    mutationFn: () => api.flows.create({ name, yaml }),
    onSuccess: () => {
      setShowNew(false);
      qc.invalidateQueries({ queryKey: ["flows"] });
    },
  });

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Operate</div>
          <h2>Flows</h2>
        </div>
        <button onClick={() => setShowNew(true)}>New flow</button>
      </div>

      {showNew && (
        <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
          <div className="label" style={{ marginBottom: 6 }}>
            New flow
          </div>
          <input
            placeholder="name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            style={{ marginBottom: 8, width: "100%" }}
          />
          <textarea
            value={yaml}
            onChange={(e) => setYaml(e.target.value)}
            rows={12}
            style={{ width: "100%" }}
          />
          <div style={{ marginTop: 12, display: "flex", gap: 8 }}>
            <button onClick={() => create.mutate()} disabled={!name.trim()}>
              Create
            </button>
            <button className="btn-ghost" onClick={() => setShowNew(false)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th style={{ width: 100 }}>Enabled</th>
              <th style={{ width: 100 }}>Version</th>
            </tr>
          </thead>
          <tbody>
            {(flows.data ?? []).map((f) => (
              <tr key={f.id}>
                <td>
                  <Link to={`/flows/${f.id}`}>{f.name}</Link>
                </td>
                <td>
                  <span className={`pill ${f.enabled ? "pill-completed" : "pill-pending"}`}>
                    {f.enabled ? "enabled" : "disabled"}
                  </span>
                </td>
                <td className="dim tnum">v{f.version}</td>
              </tr>
            ))}
            {(flows.data ?? []).length === 0 && !flows.isLoading && (
              <tr>
                <td colSpan={3} className="empty">
                  No flows yet.
                  <div className="hint">Click "New flow" to create one.</div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
