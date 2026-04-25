import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import StatusPill from "../components/status-pill";
import FileId from "../components/file-id";

export default function RunsList() {
  const [status, setStatus] = useState<string>("");
  const runs = useQuery({
    queryKey: ["runs", status],
    queryFn: () => api.runs.list(status ? { status, limit: 50 } : { limit: 50 }),
  });
  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Operate</div>
          <h2>Runs</h2>
        </div>
      </div>
      <div className="toolbar">
        <span className="label">Filter</span>
        <select value={status} onChange={(e) => setStatus(e.target.value)}>
          <option value="">All</option>
          <option value="pending">Pending</option>
          <option value="running">Running</option>
          <option value="completed">Completed</option>
          <option value="failed">Failed</option>
          <option value="skipped">Skipped</option>
          <option value="cancelled">Cancelled</option>
        </select>
      </div>
      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 60 }}>ID</th>
              <th style={{ width: 130 }}>Status</th>
              <th>File</th>
              <th style={{ width: 60 }}>Flow</th>
              <th style={{ width: 170 }}>Created</th>
              <th style={{ width: 110 }}>Duration</th>
            </tr>
          </thead>
          <tbody>
            {(runs.data ?? []).map((r) => {
              const created = r.created_at * 1000;
              const finished = (r.finished_at ?? 0) * 1000;
              const dur =
                r.finished_at && r.finished_at > r.created_at
                  ? humanizeDuration(finished - created)
                  : null;
              return (
                <tr key={r.id}>
                  <td>
                    <Link to={`/runs/${r.id}`} className="mono">
                      #{r.id}
                    </Link>
                  </td>
                  <td>
                    <StatusPill status={r.status} />
                  </td>
                  <td>
                    <Link to={`/runs/${r.id}`} style={{ color: "var(--text)" }}>
                      <FileId path={r.file_path} width={520} />
                    </Link>
                  </td>
                  <td className="dim mono">{r.flow_id}</td>
                  <td className="dim tnum">{new Date(created).toLocaleString()}</td>
                  <td className="dim tnum">{dur ?? "—"}</td>
                </tr>
              );
            })}
            {(runs.data ?? []).length === 0 && !runs.isLoading && (
              <tr>
                <td colSpan={6} className="empty">
                  No runs match this filter.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function humanizeDuration(ms: number) {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return `${m}m ${r}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}
