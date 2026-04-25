import { useLive } from "../state/live";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import StatusPill from "../components/status-pill";
import FileId from "../components/file-id";

export default function Dashboard() {
  const live = useLive();
  const recent = useQuery({
    queryKey: ["runs", "recent"],
    queryFn: () => api.runs.list({ limit: 12 }),
  });
  const failures24h = (recent.data ?? []).filter((r) => r.status === "failed").length;
  const completed24h = (recent.data ?? []).filter((r) => r.status === "completed").length;

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Operate</div>
          <h2>Dashboard</h2>
        </div>
        <div className="muted" style={{ fontSize: 11 }}>
          Live <span className="dot live" />
        </div>
      </div>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(4, minmax(0, 1fr))",
          gap: 12,
          marginBottom: 28,
        }}
      >
        <Tile label="Queue" value={live.queue.pending} />
        <Tile label="Running" value={live.queue.running} accent={live.queue.running > 0} />
        <Tile label="Completed (recent)" value={completed24h} />
        <Tile label="Failed (recent)" value={failures24h} alarm={failures24h > 0} />
      </div>

      <div className="page-header" style={{ marginBottom: 12 }}>
        <h3>Recent activity</h3>
        <Link to="/runs" className="muted" style={{ fontSize: 11 }}>
          See all →
        </Link>
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 60 }}>ID</th>
              <th style={{ width: 110 }}>Status</th>
              <th>File</th>
              <th style={{ width: 200 }}>Progress</th>
              <th style={{ width: 180 }}>Created</th>
            </tr>
          </thead>
          <tbody>
            {(recent.data ?? []).map((r) => {
              const liveStatus = live.jobStatus[r.id]?.status ?? r.status;
              const pct = live.jobProgress[r.id]?.pct;
              return (
                <tr key={r.id}>
                  <td>
                    <Link to={`/runs/${r.id}`} className="mono">
                      #{r.id}
                    </Link>
                  </td>
                  <td>
                    <StatusPill status={liveStatus} />
                  </td>
                  <td>
                    <Link to={`/runs/${r.id}`} style={{ color: "var(--text)" }}>
                      <FileId path={r.file_path} width={420} />
                    </Link>
                  </td>
                  <td>
                    {liveStatus === "running" && pct != null ? (
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <div className="progress" style={{ flex: 1 }}>
                          <div className="fill" style={{ width: `${pct}%` }} />
                        </div>
                        <span className="muted tnum" style={{ fontSize: 11 }}>
                          {pct.toFixed(1)}%
                        </span>
                      </div>
                    ) : (
                      <span className="muted">—</span>
                    )}
                  </td>
                  <td className="dim tnum">
                    {new Date(r.created_at * 1000).toLocaleString()}
                  </td>
                </tr>
              );
            })}
            {(recent.data ?? []).length === 0 && !recent.isLoading && (
              <tr>
                <td colSpan={5} className="empty">
                  No runs yet.
                  <div className="hint">
                    Trigger a webhook from Radarr / Sonarr to start one.
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

function Tile({
  label,
  value,
  accent,
  alarm,
}: {
  label: string;
  value: number;
  accent?: boolean;
  alarm?: boolean;
}) {
  return (
    <div
      className="card"
      style={{
        borderColor: alarm ? "var(--bad)" : accent ? "var(--accent)" : undefined,
      }}
    >
      <div className="card-label">{label}</div>
      <div
        className="card-value"
        style={{
          color: alarm ? "var(--bad)" : accent ? "var(--accent)" : undefined,
        }}
      >
        {value}
      </div>
    </div>
  );
}
