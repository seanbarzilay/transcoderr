import { useLive } from "../state/live";

export default function LiveProgress({ jobId }: { jobId: number }) {
  const p = useLive(s => s.jobProgress[jobId]);
  if (!p?.pct) return null;
  return (
    <div style={{ background: "rgba(255,255,255,0.06)", borderRadius: 6, height: 12, overflow: "hidden", marginBottom: 12 }}>
      <div style={{ background: "var(--accent)", height: "100%", width: `${p.pct}%` }} />
    </div>
  );
}
