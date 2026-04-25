import { create } from "zustand";
import { connectSSE } from "../api/sse";

type Live = {
  queue: { pending: number; running: number };
  jobStatus: Record<number, { status: string; label?: string }>;
  jobProgress: Record<number, { pct?: number; lastStepId?: string }>;
};
export const useLive = create<Live>(() => ({
  queue: { pending: 0, running: 0 },
  jobStatus: {},
  jobProgress: {},
}));

export function startSSE() {
  return connectSSE((e) => {
    if (e.topic === "Queue") useLive.setState({ queue: e.data });
    if (e.topic === "JobState") {
      useLive.setState((s) => ({ jobStatus: { ...s.jobStatus, [e.data.id]: { status: e.data.status, label: e.data.label } } }));
    }
    if (e.topic === "RunEvent" && e.data.kind === "progress") {
      const pct = e.data.payload?.pct;
      if (pct != null) useLive.setState((s) => ({
        jobProgress: { ...s.jobProgress, [e.data.job_id]: { pct, lastStepId: e.data.step_id } }
      }));
    }
  });
}
