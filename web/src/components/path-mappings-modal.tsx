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
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Path mappings — {workerName}</h3>
        </div>
        <div className="modal-body">
          <p style={{ marginTop: 0, fontSize: "0.9em", opacity: 0.8 }}>
            Rewrite filesystem paths between coordinator and worker. Use this
            when the worker mounts the same media at a different absolute path.
            Longest matching prefix wins.
          </p>
          {rules.length === 0 ? (
            <p>
              <em>No mappings — paths pass through unchanged.</em>
            </p>
          ) : (
            <table style={{ width: "100%" }}>
              <thead>
                <tr>
                  <th align="left">From (coordinator)</th>
                  <th align="left">To (worker)</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rules.map((r, i) => (
                  <tr key={i}>
                    <td>
                      <input
                        type="text"
                        value={r.from}
                        placeholder="/mnt/movies"
                        onChange={(e) => setRule(i, { from: e.target.value })}
                        style={{ width: "100%" }}
                      />
                    </td>
                    <td>
                      <input
                        type="text"
                        value={r.to}
                        placeholder="/data/media/movies"
                        onChange={(e) => setRule(i, { to: e.target.value })}
                        style={{ width: "100%" }}
                      />
                    </td>
                    <td>
                      <button onClick={() => removeRule(i)}>✕</button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <button onClick={addRule} style={{ marginTop: "0.5rem" }}>
            + Add mapping
          </button>
          {error && (
            <p style={{ color: "var(--color-error, red)" }}>{error}</p>
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
