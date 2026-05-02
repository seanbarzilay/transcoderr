import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";

interface Props {
  /// URL-base the operator will paste into the worker config (e.g.
  /// "wss://transcoderr.example"). Defaults to the page's origin with
  /// the scheme flipped to wss/ws.
  coordinatorUrlGuess: string;
  onClose: () => void;
}

/// Two-stage modal. Stage 1: pick a name + click Create. Stage 2: show
/// the cleartext token + a copy of the worker.toml the operator should
/// drop on the worker host. The token is shown ONCE — closing the modal
/// removes it from memory.
export default function AddWorkerForm({ coordinatorUrlGuess, onClose }: Props) {
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [created, setCreated] = useState<{ id: number; token: string; name: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.workers.create(name),
    onSuccess: (resp) => {
      setCreated({ id: resp.id, token: resp.secret_token, name });
      qc.invalidateQueries({ queryKey: ["workers"] });
    },
    onError: (e: any) => setError(e?.message ?? "create failed"),
  });

  const configToml =
    created &&
    `coordinator_url   = "${coordinatorUrlGuess}/api/worker/connect"\n` +
    `coordinator_token = "${created.token}"\n` +
    `name              = "${created.name}"\n`;

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>{created ? "Worker token" : "Add worker"}</h3>
          <button className="btn-text" onClick={onClose}>✕</button>
        </div>
        <div style={{ padding: 16 }}>
          {!created && (
            <>
              <p className="muted" style={{ fontSize: 13 }}>
                Pick a name for this worker (shown in the Workers list).
                A one-time token will be generated; drop it into the
                worker host's <code>worker.toml</code>.
              </p>
              <input
                placeholder="e.g. gpu-box-1"
                value={name}
                onChange={(e) => setName(e.target.value)}
                style={{ width: "100%", marginBottom: 8 }}
              />
              {error && (
                <p className="hint" style={{ color: "var(--bad)" }}>{error}</p>
              )}
              <div style={{ display: "flex", gap: 8 }}>
                <button
                  onClick={() => create.mutate()}
                  disabled={!name.trim() || create.isPending}
                >
                  Create
                </button>
                <button className="btn-ghost" onClick={onClose}>Cancel</button>
              </div>
            </>
          )}
          {created && (
            <>
              <p className="muted" style={{ fontSize: 13 }}>
                Copy this token now — this is the only time it will be
                shown. Save it as <code>worker.toml</code> on the worker
                host and run <code>transcoderr worker --config worker.toml</code>.
              </p>
              <pre
                style={{
                  background: "var(--surface)",
                  border: "1px solid var(--border)",
                  borderRadius: "var(--r-2)",
                  padding: 12,
                  fontSize: 12,
                  fontFamily: "var(--font-mono)",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-all",
                  marginBottom: 12,
                }}
              >
                {configToml}
              </pre>
              <div style={{ display: "flex", gap: 8 }}>
                <button
                  onClick={() => navigator.clipboard?.writeText(configToml ?? "")}
                >
                  Copy
                </button>
                <button onClick={onClose}>Done</button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
