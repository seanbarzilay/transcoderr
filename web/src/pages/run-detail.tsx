import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { api } from "../api/client";
import { useLive } from "../state/live";
import RunTimeline from "../components/run-timeline";
import LiveProgress from "../components/live-progress";

export default function RunDetail() {
  const { id } = useParams();
  const idNum = Number(id);

  // Faster polling while running so the timeline catches up to anything we
  // missed via SSE; once the run reaches a terminal status we slow down.
  const liveStatus = useLive((s) => s.jobStatus[idNum]?.status);
  const liveProgress = useLive((s) => s.jobProgress[idNum]);
  const liveEvents = useLive((s) => s.jobEvents[idNum] ?? []);

  const q = useQuery({
    queryKey: ["run", idNum],
    queryFn: () => api.runs.get(idNum),
    refetchInterval: (query) => {
      const status = liveStatus ?? query.state.data?.run.status;
      return status === "running" || status === "pending" ? 1000 : 5000;
    },
  });

  // Merge polled events (authoritative, with id/ts) and live SSE events
  // (timestamps are local clock, no id). Dedupe by (ts, kind, step_id, payload-stringified)
  // so SSE events that later show up in a poll don't double-render.
  const merged = useMemo(() => {
    const fromPoll = q.data?.events ?? [];
    const seen = new Set(
      fromPoll.map((e: any) => `${e.ts}|${e.kind}|${e.step_id ?? ""}|${JSON.stringify(e.payload ?? null)}`)
    );
    const liveOnly = liveEvents
      .filter((e) => !seen.has(`${e.ts}|${e.kind}|${e.step_id ?? ""}|${JSON.stringify(e.payload ?? null)}`))
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

  if (q.isLoading) return <div style={{ padding: 24 }}>Loading...</div>;
  if (!q.data) return <div style={{ padding: 24 }}>Not found</div>;

  const status = liveStatus ?? q.data.run.status;
  const isRunning = status === "running" || status === "pending";

  return (
    <div style={{ padding: 24 }}>
      <h2>Run #{idNum}</h2>
      <p>
        Status: <strong>{status}</strong>
        {liveProgress?.lastStepId && isRunning && (
          <> — currently <code>{liveProgress.lastStepId}</code></>
        )}
      </p>

      {isRunning && (
        <>
          <LiveProgress jobId={idNum} />
          {liveProgress?.lastFfmpegLine && (
            <div
              style={{
                fontFamily: "monospace",
                fontSize: 11,
                opacity: 0.85,
                background: "rgba(0,0,0,0.25)",
                padding: "6px 10px",
                borderRadius: 4,
                marginBottom: 12,
                whiteSpace: "nowrap",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
              title={liveProgress.lastFfmpegLine}
            >
              ffmpeg: {liveProgress.lastFfmpegLine}
            </div>
          )}
        </>
      )}

      <div style={{ marginBottom: 12 }}>
        <button onClick={() => api.runs.cancel(idNum).then(() => q.refetch())}>Cancel</button>{" "}
        <button onClick={() => api.runs.rerun(idNum)}>Rerun</button>
      </div>

      <h3>
        Timeline{" "}
        <span style={{ fontSize: 12, fontWeight: 400, opacity: 0.6 }}>
          {merged.length} events
          {isRunning && " · live"}
        </span>
      </h3>
      <RunTimeline events={merged} />
    </div>
  );
}
