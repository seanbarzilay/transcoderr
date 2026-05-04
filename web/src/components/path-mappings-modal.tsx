import { useState } from "react";
import { api } from "../api/client";

type Rule = { from: string; to: string };

type Props = {
  workerId: number;
  workerName: string;
  initialRules: Rule[];
  onClose: () => void;
  onSaved: () => void;
};

export function PathMappingsModal({
  workerId,
  workerName,
  initialRules,
  onClose,
  onSaved,
}: Props) {
  const [rules, setRules] = useState<Rule[]>(
    initialRules.length > 0 ? initialRules : [],
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function setRule(i: number, patch: Partial<Rule>) {
    setRules((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  }
  function addRule() {
    setRules((rs) => [...rs, { from: "", to: "" }]);
  }
  function removeRule(i: number) {
    setRules((rs) => rs.filter((_, idx) => idx !== i));
  }
  async function save() {
    setError(null);
    // Drop entirely-empty rows so the operator can leave a stub at the
    // bottom without it triggering a 400.
    const cleaned = rules.filter(
      (r) => r.from.trim() !== "" || r.to.trim() !== "",
    );
    // Both fields required if either is present.
    for (const r of cleaned) {
      if (r.from.trim() === "" || r.to.trim() === "") {
        setError("Each rule needs both From and To.");
        return;
      }
    }
    setSaving(true);
    try {
      await api.workers.updatePathMappings(workerId, cleaned);
      onSaved();
      onClose();
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={saving ? undefined : onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Path mappings — {workerName}</h3>
        </div>
        <div className="modal-body" style={{ display: "flex", flexDirection: "column", gap: "1rem" }}>
          <p style={{ margin: 0, fontSize: "0.9em", opacity: 0.8 }}>
            Rewrite filesystem paths between coordinator and worker. Use this
            when the worker mounts the same media at a different absolute path.
            Longest matching prefix wins.
          </p>
          {rules.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: "0.5rem" }}>
              <div
                style={{
                  display: "flex",
                  gap: "0.75rem",
                  fontSize: "0.8em",
                  opacity: 0.6,
                  textTransform: "uppercase",
                  letterSpacing: "0.05em",
                }}
              >
                <span style={{ flex: 1 }}>From (coordinator)</span>
                <span style={{ flex: 1 }}>To (worker)</span>
                <span style={{ width: "2rem" }} />
              </div>
              {rules.map((r, i) => (
                <div
                  key={i}
                  style={{
                    display: "flex",
                    gap: "0.75rem",
                    alignItems: "center",
                  }}
                >
                  <input
                    type="text"
                    value={r.from}
                    placeholder="/mnt/movies"
                    onChange={(e) => setRule(i, { from: e.target.value })}
                    style={{ flex: 1, minWidth: 0 }}
                  />
                  <input
                    type="text"
                    value={r.to}
                    placeholder="/data/media/movies"
                    onChange={(e) => setRule(i, { to: e.target.value })}
                    style={{ flex: 1, minWidth: 0 }}
                  />
                  <button
                    className="btn-ghost"
                    onClick={() => removeRule(i)}
                    title="Remove mapping"
                    style={{ width: "2rem", padding: "0.25rem 0", flexShrink: 0 }}
                  >
                    ✕
                  </button>
                </div>
              ))}
            </div>
          )}
          {rules.length === 0 && (
            <p style={{ margin: 0, opacity: 0.6 }}>
              <em>No mappings — paths pass through unchanged.</em>
            </p>
          )}
          <div>
            <button className="btn-ghost" onClick={addRule}>
              + Add mapping
            </button>
          </div>
          {error && (
            <p style={{ margin: 0, color: "var(--color-error, red)" }}>{error}</p>
          )}
        </div>
        <div className="modal-footer">
          <button onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button onClick={save} disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
