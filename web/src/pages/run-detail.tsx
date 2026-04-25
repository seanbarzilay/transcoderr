import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import RunTimeline from "../components/run-timeline";
import LiveProgress from "../components/live-progress";

export default function RunDetail() {
  const { id } = useParams();
  const idNum = Number(id);
  const q = useQuery({ queryKey: ["run", idNum], queryFn: () => api.runs.get(idNum), refetchInterval: 2000 });
  if (q.isLoading) return <div style={{ padding: 24 }}>Loading...</div>;
  if (!q.data) return <div style={{ padding: 24 }}>Not found</div>;
  return (
    <div style={{ padding: 24 }}>
      <h2>Run #{idNum}</h2>
      <p>Status: <strong>{q.data.run.status}</strong></p>
      {q.data.run.status === "running" && <LiveProgress jobId={idNum} />}
      <div style={{ marginBottom: 12 }}>
        <button onClick={() => api.runs.cancel(idNum).then(() => q.refetch())}>Cancel</button>{" "}
        <button onClick={() => api.runs.rerun(idNum)}>Rerun</button>
      </div>
      <h3>Timeline</h3>
      <RunTimeline events={q.data.events} />
    </div>
  );
}
