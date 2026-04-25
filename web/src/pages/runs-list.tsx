import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api } from "../api/client";

export default function RunsList() {
  const [status, setStatus] = useState<string>("");
  const runs = useQuery({
    queryKey: ["runs", status],
    queryFn: () => api.runs.list(status ? { status } : undefined),
  });
  return (
    <div style={{ padding: 24 }}>
      <h2>Runs</h2>
      <select value={status} onChange={e => setStatus(e.target.value)} style={{ marginBottom: 12 }}>
        <option value="">All</option>
        <option value="pending">Pending</option>
        <option value="running">Running</option>
        <option value="completed">Completed</option>
        <option value="failed">Failed</option>
        <option value="skipped">Skipped</option>
      </select>
      <table>
        <thead><tr><th>ID</th><th>Flow</th><th>Status</th><th>Created</th></tr></thead>
        <tbody>
          {(runs.data ?? []).map(r => (
            <tr key={r.id}>
              <td><Link to={`/runs/${r.id}`}>{r.id}</Link></td>
              <td>{r.flow_id}</td>
              <td>{r.status}</td>
              <td>{new Date(r.created_at*1000).toLocaleString()}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
