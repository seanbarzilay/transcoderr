import { create } from "zustand";
import { connectSSE } from "../api/sse";
import { asRecord } from "../lib/records";

export type LiveRunEvent = {
  ts: number;
  job_id: number;
  step_id?: string;
  kind: string;
  payload?: unknown;
  worker_id?: number;
  worker_name?: string;
};

type Live = {
  queue: { pending: number; running: number };
  jobStatus: Record<number, { status: string; label?: string }>;
  jobProgress: Record<number, { pct?: number; lastStepId?: string; lastFfmpegLine?: string }>;
  /// Tail of run events per job, capped at MAX_TAIL most-recent entries.
  jobEvents: Record<number, LiveRunEvent[]>;
};

const MAX_TAIL = 200;

export const useLive = create<Live>(() => ({
  queue: { pending: 0, running: 0 },
  jobStatus: {},
  jobProgress: {},
  jobEvents: {},
}));

function appendEvent(jobId: number, ev: LiveRunEvent) {
  useLive.setState((s) => {
    const prev = s.jobEvents[jobId] ?? [];
    const next = prev.concat(ev);
    if (next.length > MAX_TAIL) next.splice(0, next.length - MAX_TAIL);
    return { jobEvents: { ...s.jobEvents, [jobId]: next } };
  });
}

export function startSSE() {
  return connectSSE((e) => {
    if (e.topic === "Queue") useLive.setState({ queue: e.data });
    if (e.topic === "JobState") {
      useLive.setState((s) => ({
        jobStatus: { ...s.jobStatus, [e.data.id]: { status: e.data.status, label: e.data.label } },
      }));
    }
    if (e.topic === "RunEvent") {
      const ev: LiveRunEvent = {
        ts: Math.floor(Date.now() / 1000),
        job_id: e.data.job_id,
        step_id: e.data.step_id,
        kind: e.data.kind,
        payload: e.data.payload,
        worker_id: e.data.worker_id,
        worker_name: e.data.worker_name,
      };
      appendEvent(e.data.job_id, ev);

      if (e.data.kind === "progress") {
        const pct = asRecord(e.data.payload).pct;
        if (typeof pct === "number") {
          useLive.setState((s) => ({
            jobProgress: {
              ...s.jobProgress,
              [e.data.job_id]: {
                ...s.jobProgress[e.data.job_id],
                pct,
                lastStepId: e.data.step_id,
              },
            },
          }));
        }
      }
      if (e.data.kind === "log") {
        const msg = asRecord(e.data.payload).msg;
        if (typeof msg === "string" && msg.startsWith("ffmpeg: ")) {
          useLive.setState((s) => ({
            jobProgress: {
              ...s.jobProgress,
              [e.data.job_id]: {
                ...s.jobProgress[e.data.job_id],
                lastFfmpegLine: msg.slice("ffmpeg: ".length),
              },
            },
          }));
        }
      }
    }
  });
}
