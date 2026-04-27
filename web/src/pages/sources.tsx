import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Source } from "../types";

const AUTO_KINDS = ["radarr", "sonarr", "lidarr"] as const;

function isAutoKind(kind: string): boolean {
  return (AUTO_KINDS as readonly string[]).includes(kind);
}

function isAutoSource(src: Source): boolean {
  return isAutoKind(src.kind) && src.config?.arr_notification_id != null;
}

function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}

export default function Sources() {
  const qc = useQueryClient();
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });

  const [kind, setKind] = useState<string>("radarr");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [secretToken, setSecretToken] = useState("");
  const [config, setConfig] = useState("{}");
  const [formError, setFormError] = useState<string | null>(null);

  const isAutoProvision = isAutoKind(kind);

  const create = useMutation({
    mutationFn: (body: any) => api.sources.create(body),
    onSuccess: () => {
      setName("");
      setBaseUrl("");
      setApiKey("");
      setSecretToken("");
      setConfig("{}");
      setFormError(null);
      qc.invalidateQueries({ queryKey: ["sources"] });
    },
    onError: (e: any) => {
      setFormError(e?.message ?? String(e));
    },
  });

  const del = useMutation({
    mutationFn: (id: number) => api.sources.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sources"] }),
  });

  const submit = () => {
    setFormError(null);
    if (isAutoProvision) {
      create.mutate({
        kind,
        name,
        config: { base_url: baseUrl, api_key: apiKey },
        secret_token: "",
      });
    } else {
      let parsed: any;
      try {
        parsed = JSON.parse(config || "{}");
      } catch (e: any) {
        setFormError(`Invalid config JSON: ${e?.message ?? e}`);
        return;
      }
      create.mutate({
        kind,
        name,
        config: parsed,
        secret_token: secretToken,
      });
    }
  };

  const canSubmit = (() => {
    if (!name.trim()) return false;
    if (isAutoProvision) return baseUrl.trim() !== "" && apiKey.trim() !== "";
    return secretToken.trim() !== "";
  })();

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Sources</h2>
        </div>
      </div>

      <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
        <div className="label" style={{ marginBottom: 8 }}>
          Add source
        </div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            <option value="radarr">radarr</option>
            <option value="sonarr">sonarr</option>
            <option value="lidarr">lidarr</option>
            <option value="webhook">webhook</option>
          </select>
          <input
            placeholder="name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          {isAutoProvision ? (
            <>
              <input
                placeholder={`base url (e.g. http://${kind}:${
                  kind === "radarr" ? "7878" : kind === "sonarr" ? "8989" : "8686"
                })`}
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                style={{ flex: 1, minWidth: 240 }}
              />
              <input
                type="password"
                placeholder="api key"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
              />
            </>
          ) : (
            <>
              <input
                placeholder="secret token"
                value={secretToken}
                onChange={(e) => setSecretToken(e.target.value)}
              />
              <input
                placeholder="config json (e.g. {})"
                value={config}
                onChange={(e) => setConfig(e.target.value)}
                style={{ flex: 1, minWidth: 240 }}
              />
            </>
          )}
          <button onClick={submit} disabled={!canSubmit || create.isPending}>
            Add
          </button>
        </div>
        {isAutoProvision ? (
          <p className="hint">
            Transcoderr will create the webhook in {capitalize(kind)} for you.
            The connection token is generated automatically.
          </p>
        ) : (
          <p className="hint">
            Add a webhook in your tool's settings pointing at{" "}
            <code>{`{public_url}/webhook/${name || "{name}"}`}</code> with the
            secret token above as the password.
          </p>
        )}
        {formError && (
          <p className="hint" style={{ color: "var(--bad)" }}>
            {formError}
          </p>
        )}
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 100 }}>Kind</th>
              <th style={{ width: 90 }}>Mode</th>
              <th style={{ width: 180 }}>Name</th>
              <th>Webhook URL</th>
              <th style={{ width: 110 }}></th>
            </tr>
          </thead>
          <tbody>
            {(sources.data ?? []).map((s: Source) => (
              <tr key={s.id}>
                <td>
                  <span className="label">{s.kind}</span>
                </td>
                <td>
                  <span
                    className={`badge badge-${
                      isAutoSource(s) ? "auto" : "manual"
                    }`}
                  >
                    {isAutoSource(s) ? "auto" : "manual"}
                  </span>
                </td>
                <td>{s.name}</td>
                <td className="mono dim">
                  {s.kind === "webhook"
                    ? `/webhook/${s.name}`
                    : `/webhook/${s.kind}`}
                </td>
                <td>
                  <button
                    className="btn-danger"
                    onClick={() => del.mutate(s.id)}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
            {(sources.data ?? []).length === 0 && !sources.isLoading && (
              <tr>
                <td colSpan={5} className="empty">
                  No sources configured.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
