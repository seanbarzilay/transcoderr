import type { RunEvent } from "../types";

export default function RunTimeline({ events }: { events: RunEvent[] }) {
  return (
    <ol style={{ listStyle: "none", padding: 0 }}>
      {events.map(e => (
        <li key={e.id} style={{ borderLeft: "2px solid #444", paddingLeft: 12, marginBottom: 6 }}>
          <code style={{ opacity: 0.7 }}>{new Date(e.ts*1000).toISOString()}</code>{" "}
          <strong>{e.kind}</strong>{" "}
          {e.step_id && <span style={{ opacity: 0.8 }}>· {e.step_id}</span>}
          {e.payload && <pre style={{ marginTop: 4 }}>{JSON.stringify(e.payload, null, 2)}</pre>}
        </li>
      ))}
    </ol>
  );
}
