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
  const [tab, setTab] = useState<"editor" | "test">("editor");
  const [parseResult, setParseResult] = useState<any>(null);
  const [filePath, setFilePath] = useState("");
  const [dryResult, setDryResult] = useState<any>(null);

  useEffect(() => {
    if (flow.data && yaml === "") setYaml(flow.data.yaml_source);
  }, [flow.data]);

  useEffect(() => {
    const t = setTimeout(async () => {
      if (!yaml) return;
      try {
        setParseResult(await api.flows.parse(yaml));
      } catch {
        /* noop */
      }
    }, 200);
    return () => clearTimeout(t);
  }, [yaml]);

  const save = useMutation({
    mutationFn: () => api.flows.update(idNum, { yaml }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["flow", idNum] }),
  });

  return (
    <div className="page">
      {typeof window !== "undefined" && window.innerWidth < 1024 && (
        <div
          className="surface"
          style={{
            padding: "8px 12px",
            marginBottom: 12,
            borderColor: "var(--bad)",
            color: "var(--bad)",
            fontSize: 11,
            letterSpacing: "0.05em",
            textTransform: "uppercase",
          }}
        >
          ⚠ Flow editor is desktop-only — open on a wider screen
        </div>
      )}

      <div className="page-header">
        <div>
          <div className="crumb">Operate / Flows</div>
          <h2>{flow.data?.name ?? "—"}</h2>
        </div>
        <div style={{ display: "flex", gap: 8 }}>
          {(["editor", "test"] as const).map((t) => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={tab === t ? "" : "btn-ghost"}
            >
              {t}
            </button>
          ))}
          <button onClick={() => save.mutate()} disabled={save.isPending}>
            Save
          </button>
        </div>
      </div>

      {tab === "editor" && (
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
          <div className="surface" style={{ padding: 0, overflow: "hidden" }}>
            <YamlEditor value={yaml} onChange={setYaml} />
          </div>
          <div className="surface" style={{ padding: 16 }}>
            <div className="label" style={{ marginBottom: 10 }}>
              Mirror
            </div>
            {parseResult?.ok ? (
              <FlowMirror parsed={parseResult.parsed} />
            ) : (
              <pre style={{ color: "var(--bad)" }}>{parseResult?.error ?? ""}</pre>
            )}
          </div>
        </div>
      )}

      {tab === "test" && (
        <div className="surface" style={{ padding: 16 }}>
          <div className="label" style={{ marginBottom: 8 }}>
            Dry run against file
          </div>
          <div style={{ display: "flex", gap: 8, marginBottom: 12 }}>
            <input
              value={filePath}
              onChange={(e) => setFilePath(e.target.value)}
              placeholder="/path/to/file.mkv"
              style={{ flex: 1 }}
            />
            <button onClick={async () => setDryResult(await api.dryRun({ yaml, file_path: filePath }))}>
              Test
            </button>
          </div>
          {dryResult && (
            <ol style={{ paddingLeft: 18, margin: 0 }}>
              {dryResult.steps.map((s: any, i: number) => (
                <li key={i} style={{ marginBottom: 4 }}>
                  <span className="label" style={{ marginRight: 8 }}>
                    {s.kind}
                  </span>
                  <span className="mono">{s.use_or_label}</span>
                </li>
              ))}
            </ol>
          )}
        </div>
      )}
    </div>
  );
}
