import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";
import { errorMessage } from "../../lib/errors";
import hevcNormalizeYaml from "../../templates/hevc-normalize.yaml?raw";

interface Props {
  onCreated: () => void;
  onSkip: () => void;
}

export default function FlowStep({ onCreated, onSkip }: Props) {
  const qc = useQueryClient();
  const [name, setName] = useState("hevc-normalize");
  const [yaml, setYaml] = useState(hevcNormalizeYaml);
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.flows.create({ name, yaml }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["flows"] });
      onCreated();
    },
    onError: (e: unknown) => setError(errorMessage(e, "failed to create flow")),
  });

  const disabled = create.isPending || !name.trim() || !yaml.trim();

  return (
    <div className="wizard-step">
      <h4>Create your first flow</h4>
      <p className="muted">
        Pre-loaded with <code>hevc-normalize</code>: re-encodes anything that
        isn't already HEVC to x265 (CRF 19, fast preset), ensures an English
        AC3 6ch audio track, drops cover-art and data streams, and replaces
        the original file. Edit if you want, or save as-is.
      </p>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
        <label style={{ fontSize: 12, color: "var(--text-dim)" }}>name</label>
        <input
          value={name}
          onChange={e => setName(e.target.value)}
          style={{ flex: 1 }}
        />
      </div>
      <textarea
        value={yaml}
        onChange={e => setYaml(e.target.value)}
        spellCheck={false}
        style={{
          width: "100%",
          minHeight: 280,
          fontFamily: "var(--font-mono)",
          fontSize: 12,
          background: "var(--surface)",
          color: "var(--text)",
          border: "1px solid var(--border)",
          borderRadius: "var(--r-2)",
          padding: 8,
          resize: "vertical",
        }}
      />
      {error && (
        <p className="hint" style={{ color: "var(--bad)" }}>{error}</p>
      )}
      <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
        <button onClick={() => create.mutate()} disabled={disabled}>
          Save flow
        </button>
        <button className="btn-ghost" onClick={onSkip}>Skip this step</button>
      </div>
    </div>
  );
}
