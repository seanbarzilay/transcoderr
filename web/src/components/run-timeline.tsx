import type { RunEvent } from "../types";

export default function RunTimeline({ events }: { events: RunEvent[] }) {
  return (
    <ol className="timeline">
      {events.map((e) => {
        const ts = new Date(e.ts * 1000);
        const time = ts.toLocaleTimeString(undefined, { hour12: false });
        const payload = renderPayload(e);
        return (
          <li key={e.id} className="timeline-row">
            <div className="ts">{time}</div>
            <div className={`kind kind-${e.kind}`}>{e.kind.replace(/_/g, " ")}</div>
            <div className="step mono">{e.step_id ?? ""}</div>
            <div className="payload">{payload}</div>
          </li>
        );
      })}
    </ol>
  );
}

function renderPayload(e: RunEvent): string | null {
  const p = e.payload as Record<string, unknown> | null | undefined;
  if (!p) return null;
  if (typeof p["msg"] === "string") return p["msg"] as string;
  if (typeof p["error"] === "string") return p["error"] as string;
  if (typeof p["pct"] === "number") return `${(p["pct"] as number).toFixed(1)}%`;
  if (typeof p["expr"] === "string") {
    const result = "result" in p ? `  →  ${p["result"]}` : "";
    return `${p["expr"]}${result}`;
  }
  if (typeof p["use"] === "string") {
    const attempt = p["attempt"] != null ? `  attempt=${p["attempt"]}` : "";
    return `use=${p["use"]}${attempt}`;
  }
  try {
    return JSON.stringify(p);
  } catch {
    return null;
  }
}
