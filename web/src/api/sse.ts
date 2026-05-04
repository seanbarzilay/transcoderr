type Event =
  | { topic: "JobState"; data: { id: number; status: string; label?: string } }
  | { topic: "RunEvent"; data: { job_id: number; step_id?: string; kind: string; payload: unknown; worker_id?: number; worker_name?: string } }
  | { topic: "Queue";    data: { pending: number; running: number } };

export function connectSSE(onEvent: (e: Event) => void): () => void {
  const es = new EventSource("/api/stream", { withCredentials: true });
  es.onmessage = (m) => {
    try { onEvent(JSON.parse(m.data)); } catch { return; }
  };
  return () => es.close();
}
