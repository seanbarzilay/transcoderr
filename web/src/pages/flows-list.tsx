import { useEffect, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api } from "../api/client";
import type { FlowValidationIssue } from "../types";

const STARTER_YAML = `name: example
triggers:
  - radarr: [downloaded]
steps:
  - use: probe
  - use: plan.init
  - use: plan.execute
  - use: output
    with: { mode: replace }
`;

export default function FlowsList() {
  const qc = useQueryClient();
  const flows = useQuery({ queryKey: ["flows"], queryFn: api.flows.list });
  const [showNew, setShowNew] = useState(false);
  const [name, setName] = useState("");
  const [yaml, setYaml] = useState(STARTER_YAML);
  const [issues, setIssues] = useState<FlowValidationIssue[] | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [validateError, setValidateError] = useState<string | null>(null);

  useEffect(() => {
    setIssues(null);
    setAcknowledged(false);
  }, [yaml]);

  const create = useMutation({
    mutationFn: () => api.flows.create({ name, yaml }),
    onSuccess: () => {
      setShowNew(false);
      setIssues(null);
      setAcknowledged(false);
      qc.invalidateQueries({ queryKey: ["flows"] });
    },
  });

  const onCreate = async () => {
    setValidateError(null);
    if (!acknowledged) {
      try {
        const report = await api.flows.validate(yaml);
        const compileIssues = report.issues.filter(
          (i) => i.kind !== "yaml_parse_error",
        );
        setIssues(compileIssues);
        setAcknowledged(true);
        if (compileIssues.length > 0) return;
      } catch (e: unknown) {
        setValidateError(e instanceof Error ? e.message : String(e));
        return;
      }
    }
    create.mutate();
  };

  const hasWarnings = (issues?.length ?? 0) > 0;

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Operate</div>
          <h2>Flows</h2>
        </div>
        <button onClick={() => setShowNew(true)}>New flow</button>
      </div>

      {showNew && (
        <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
          <div className="label" style={{ marginBottom: 6 }}>
            New flow
          </div>
          <input
            placeholder="name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            style={{ marginBottom: 8, width: "100%" }}
          />
          <textarea
            value={yaml}
            onChange={(e) => setYaml(e.target.value)}
            rows={12}
            style={{ width: "100%" }}
          />
          {hasWarnings && (
            <div
              className="surface"
              style={{
                marginTop: 8,
                padding: 10,
                borderColor: "var(--bad)",
                background: "var(--bad-soft)",
                fontSize: 12,
              }}
            >
              <div style={{ color: "var(--bad)", marginBottom: 6, fontWeight: 600 }}>
                ⚠ {issues!.length} compile issue{issues!.length === 1 ? "" : "s"} —
                click Create again to commit anyway, or edit the YAML to fix.
              </div>
              <ul style={{ paddingLeft: 16, margin: 0 }}>
                {issues!.map((iss, i) => (
                  <li key={i} style={{ marginBottom: 4 }}>
                    <span className="mono">{iss.path}</span>{" "}
                    <span style={{ color: "var(--text-dim)" }}>— {iss.message}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {validateError && (
            <p className="hint" style={{ color: "var(--bad)" }}>{validateError}</p>
          )}
          <div style={{ marginTop: 12, display: "flex", gap: 8 }}>
            <button onClick={onCreate} disabled={!name.trim() || create.isPending}>
              {hasWarnings ? "Create anyway" : "Create"}
            </button>
            <button className="btn-ghost" onClick={() => setShowNew(false)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th style={{ width: 100 }}>Enabled</th>
              <th style={{ width: 100 }}>Version</th>
            </tr>
          </thead>
          <tbody>
            {(flows.data ?? []).map((f) => (
              <tr key={f.id}>
                <td>
                  <Link to={`/flows/${f.id}`}>{f.name}</Link>
                </td>
                <td>
                  <span className={`pill ${f.enabled ? "pill-completed" : "pill-pending"}`}>
                    {f.enabled ? "enabled" : "disabled"}
                  </span>
                </td>
                <td className="dim tnum">v{f.version}</td>
              </tr>
            ))}
            {(flows.data ?? []).length === 0 && !flows.isLoading && (
              <tr>
                <td colSpan={3} className="empty">
                  No flows yet.
                  <div className="hint">Click "New flow" to create one.</div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
