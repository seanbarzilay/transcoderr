import { useLive } from "../state/live";

export default function LiveProgress({ jobId }: { jobId: number }) {
  const p = useLive((s) => s.jobProgress[jobId]);
  if (!p?.pct) return null;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 12 }}>
      <div className="progress" style={{ flex: 1, height: 8 }}>
        <div className="fill" style={{ width: `${p.pct}%` }} />
      </div>
      <span className="dim tnum" style={{ fontSize: 11, minWidth: 56, textAlign: "right" }}>
        {p.pct.toFixed(1)}%
      </span>
    </div>
  );
}
