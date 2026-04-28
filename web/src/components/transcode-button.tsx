import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { TranscodeReq, TranscodeResp } from "../types-arr";

interface Props {
  sourceId: number;
  payload: TranscodeReq;
  disabled?: boolean;
  disabledReason?: string;
}

export default function TranscodeButton({
  sourceId,
  payload,
  disabled,
  disabledReason,
}: Props) {
  const qc = useQueryClient();
  const [result, setResult] = useState<TranscodeResp | null>(null);
  const [error, setError] = useState<string | null>(null);

  const mut = useMutation({
    mutationFn: () => api.arr.transcode(sourceId, payload),
    onSuccess: (data) => {
      setResult(data);
      setError(null);
      qc.invalidateQueries({ queryKey: ["runs"] });
    },
    onError: (e: any) => {
      setError(e?.message ?? String(e));
      setResult(null);
    },
  });

  if (disabled) {
    return (
      <button type="button" className="mock-button" disabled title={disabledReason ?? ""}>
        Transcode
      </button>
    );
  }

  return (
    <div className="transcode-action">
      <button
        type="button"
        className="mock-button"
        disabled={mut.isPending}
        onClick={() => mut.mutate()}
      >
        {mut.isPending ? "Queueing…" : "Transcode"}
      </button>
      {result && (
        <div className="hint" style={{ color: "var(--ok)" }}>
          Queued {result.runs.length} run{result.runs.length === 1 ? "" : "s"}:{" "}
          {result.runs.map((r, i) => (
            <span key={r.run_id}>
              {i > 0 && ", "}
              <a href={`/runs/${r.run_id}`}>
                {r.flow_name} #{r.run_id}
              </a>
            </span>
          ))}
        </div>
      )}
      {error && (
        <div className="hint" style={{ color: "var(--bad)" }}>
          {error}
        </div>
      )}
    </div>
  );
}
