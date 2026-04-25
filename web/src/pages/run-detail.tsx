import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { api } from "../api/client";
import { useLive, type LiveRunEvent } from "../state/live";
import RunTimeline from "../components/run-timeline";
import LiveProgress from "../components/live-progress";
import StatusPill from "../components/status-pill";
import { basename } from "../components/file-id";

const EMPTY_EVENTS: LiveRunEvent[] = [];

export default function RunDetail() {
  const { id } = useParams();
  const idNum = Number(id);

  const liveStatus = useLive((s) => s.jobStatus[idNum]?.status);
  const liveProgress = useLive((s) => s.jobProgress[idNum]);
  const liveEvents = useLive((s) => s.jobEvents[idNum] ?? EMPTY_EVENTS);

  const q = useQuery({
    queryKey: ["run", idNum],
    queryFn: () => api.runs.get(idNum),
    refetchInterval: (query) => {
      const status = liveStatus ?? query.state.data?.run.status;
      return status === "running" || status === "pending" ? 15000 : false;
    },
  });

  const merged = useMemo(() => {
    const fromPoll = q.data?.events ?? [];
    const seen = new Set(
      fromPoll.map(
        (e: any) =>
          `${e.ts}|${e.kind}|${e.step_id ?? ""}|${JSON.stringify(e.payload ?? null)}`
      )
    );
    const liveOnly = liveEvents
      .filter(
        (e) =>
          !seen.has(
            `${e.ts}|${e.kind}|${e.step_id ?? ""}|${JSON.stringify(e.payload ?? null)}`
          )
      )
      .map((e, i) => ({
        id: -1 - i,
        job_id: e.job_id,
        ts: e.ts,
        step_id: e.step_id,
        kind: e.kind,
        payload: e.payload,
      }));
    return [...fromPoll, ...liveOnly].sort((a, b) => b.ts - a.ts);
  }, [q.data?.events, liveEvents]);

  if (q.isLoading)
    return (
      <div className="page">
        <span className="muted">Loading…</span>
      </div>
    );
  if (!q.data)
    return (
      <div className="page">
        <span className="muted">Run not found.</span>
      </div>
    );

  const status = liveStatus ?? q.data.run.status;
  const isRunning = status === "running" || status === "pending";
  const created = q.data.run.created_at * 1000;
  const finished = (q.data.run.finished_at ?? 0) * 1000;
  const duration = finished > created ? humanizeDuration(finished - created) : null;

  return (
    <div className="page">
      <div className="page-header">
        <div style={{ minWidth: 0, flex: 1 }}>
          <div className="crumb">
            Operate / Runs / Run #{idNum}
          </div>
          <h2
            title={q.data.run.file_path}
            style={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {basename(q.data.run.file_path) || `Run #${idNum}`}
          </h2>
          <div
            className="dim mono"
            title={q.data.run.file_path}
            style={{
              fontSize: 11,
              marginTop: 4,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {q.data.run.file_path}
          </div>
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          <button
            className="btn-ghost"
            onClick={() => api.runs.rerun(idNum)}
            title="Rerun this job"
          >
            Rerun
          </button>
          <button
            className="btn-danger"
            onClick={() => api.runs.cancel(idNum).then(() => q.refetch())}
            disabled={!isRunning}
          >
            Cancel
          </button>
        </div>
      </div>

      <div
        className="surface"
        style={{ padding: 16, marginBottom: 20, display: "grid", gridTemplateColumns: "repeat(4, 1fr)", gap: 16 }}
      >
        <Field label="Status" value={<StatusPill status={status} />} />
        <Field label="Flow" value={<span className="mono">{q.data.run.flow_id}</span>} />
        <Field
          label="Started"
          value={
            <span className="dim tnum">
              {new Date(created).toLocaleString()}
            </span>
          }
        />
        <Field
          label="Duration"
          value={<span className="dim tnum">{duration ?? (isRunning ? "…" : "—")}</span>}
        />
      </div>

      {isRunning && (
        <div style={{ marginBottom: 20 }}>
          <div className="label" style={{ marginBottom: 8 }}>
            Now: {liveProgress?.lastStepId ?? "—"}
          </div>
          <LiveProgress jobId={idNum} />
          {liveProgress?.lastFfmpegLine && (
            <div className="tail" title={liveProgress.lastFfmpegLine}>
              {liveProgress.lastFfmpegLine}
            </div>
          )}
        </div>
      )}

      <div className="page-header" style={{ marginBottom: 8 }}>
        <h3>
          Timeline <span className="muted" style={{ fontWeight: 400 }}>
            {merged.length} events{isRunning && " · live"}
          </span>
        </h3>
      </div>
      <div className="surface" style={{ padding: "0 16px" }}>
        <RunTimeline events={merged} />
      </div>
    </div>
  );
}

function Field({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div>
      <div className="label" style={{ marginBottom: 6 }}>
        {label}
      </div>
      <div>{value}</div>
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
