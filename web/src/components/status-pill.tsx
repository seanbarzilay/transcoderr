/// Status pill — colored, monospace small-caps. Used everywhere a job /
/// run / step status is rendered.

const KNOWN = new Set([
  "pending",
  "running",
  "completed",
  "failed",
  "skipped",
  "cancelled",
]);

export default function StatusPill({ status }: { status?: string }) {
  const s = (status ?? "").toLowerCase();
  const cls = KNOWN.has(s) ? `pill pill-${s}` : "pill pill-pending";
  return (
    <span className={cls}>
      {s === "running" && <span className="dot live" />}
      {s || "—"}
    </span>
  );
}
