import { useLive } from "../state/live";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";

export default function Dashboard() {
  const live = useLive();
  const recent = useQuery({ queryKey: ["runs", "recent"], queryFn: () => api.runs.list({ limit: 10 }) });
  const failures24h = (recent.data ?? []).filter(r => r.status === "failed").length;

  return (
    <div style={{ padding: 24 }}>
      <h2>Dashboard</h2>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(4, 1fr)", gap: 12 }}>
        <Tile label="Queue" value={live.queue.pending} />
        <Tile label="Running" value={live.queue.running} />
        <Tile label="Recent runs" value={recent.data?.length ?? 0} />
        <Tile label="Failures (24h)" value={failures24h} />
      </div>
      <h3 style={{ marginTop: 24 }}>Recent activity</h3>
      <table>
        <thead><tr><th>ID</th><th>Status</th><th>Progress</th><th>Created</th></tr></thead>
        <tbody>
          {(recent.data ?? []).map(r => (
            <tr key={r.id}>
              <td><a href={`/runs/${r.id}`}>{r.id}</a></td>
              <td>{live.jobStatus[r.id]?.status ?? r.status}</td>
              <td>{live.jobProgress[r.id]?.pct ? `${live.jobProgress[r.id]!.pct!.toFixed(1)}%` : ""}</td>
              <td>{new Date(r.created_at*1000).toLocaleString()}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Tile({ label, value }: { label: string; value: number }) {
  return (
    <div style={{ background: "rgba(255,255,255,0.05)", padding: 16, borderRadius: 8 }}>
      <div style={{ fontSize: 12, opacity: 0.7 }}>{label}</div>
      <div style={{ fontSize: 32, fontWeight: 700 }}>{value}</div>
    </div>
  );
}
