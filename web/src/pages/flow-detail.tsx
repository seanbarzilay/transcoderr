import { useParams } from "react-router-dom";
import { useState, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import YamlEditor from "../components/yaml-editor";
import FlowMirror from "../components/flow-mirror";

export default function FlowDetail() {
  const { id } = useParams();
  const qc = useQueryClient();
  const idNum = Number(id);
  const flow = useQuery({ queryKey: ["flow", idNum], queryFn: () => api.flows.get(idNum) });
  const [yaml, setYaml] = useState<string>("");
  const [tab, setTab] = useState<"editor"|"test">("editor");
  const [parseResult, setParseResult] = useState<any>(null);
  const [filePath, setFilePath] = useState("");
  const [dryResult, setDryResult] = useState<any>(null);

  useEffect(() => { if (flow.data && yaml === "") setYaml(flow.data.yaml_source); }, [flow.data]);

  // Debounced live parse
  useEffect(() => {
    const t = setTimeout(async () => {
      if (!yaml) return;
      try { setParseResult(await api.flows.parse(yaml)); } catch {}
    }, 200);
    return () => clearTimeout(t);
  }, [yaml]);

  const save = useMutation({
    mutationFn: () => api.flows.update(idNum, { yaml }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["flow", idNum] }),
  });

  return (
    <div style={{ padding: 24 }}>
      <h2>{flow.data?.name}</h2>
      <div style={{ display: "flex", gap: 12, marginBottom: 12 }}>
        {(["editor","test"] as const).map(t =>
          <button key={t} onClick={() => setTab(t)} style={{ background: tab === t ? "var(--accent)" : "transparent", border: "1px solid rgba(255,255,255,0.2)" }}>{t}</button>
        )}
        <button onClick={() => save.mutate()} disabled={save.isPending}>Save</button>
      </div>
      {tab === "editor" && (
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
          <YamlEditor value={yaml} onChange={setYaml} />
          <div>
            {parseResult?.ok
              ? <FlowMirror parsed={parseResult.parsed} />
              : <pre style={{ color: "#f88" }}>{parseResult?.error ?? ""}</pre>}
          </div>
        </div>
      )}
      {tab === "test" && (
        <div>
          <input value={filePath} onChange={e => setFilePath(e.target.value)} placeholder="/path/to/file.mkv" style={{ width: "60%", marginRight: 8 }} />
          <button onClick={async () => setDryResult(await api.dryRun({ yaml, file_path: filePath }))}>Test</button>
          {dryResult && <ol style={{ marginTop: 12 }}>{dryResult.steps.map((s: any, i: number) => <li key={i}>{s.kind}: {s.use_or_label}</li>)}</ol>}
        </div>
      )}
    </div>
  );
}
