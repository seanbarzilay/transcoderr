import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api/client";
import { errorMessage } from "../../lib/errors";
import type { JsonObject } from "../../types";

const AUTO_KINDS = ["radarr", "sonarr", "lidarr"] as const;

function isAutoKind(kind: string): boolean {
  return (AUTO_KINDS as readonly string[]).includes(kind);
}

function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}

interface Props {
  /// Called with the new source's id after a successful create. Use this to
  /// advance the setup wizard, navigate somewhere, etc.
  onCreated?: (id: number) => void;
}

export default function AddSourceForm({ onCreated }: Props) {
  const qc = useQueryClient();
  const [kind, setKind] = useState<string>("radarr");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [secretToken, setSecretToken] = useState("");
  const [config, setConfig] = useState("{}");
  const [formError, setFormError] = useState<string | null>(null);

  const isAutoProvision = isAutoKind(kind);

  const create = useMutation({
    mutationFn: (body: JsonObject) => api.sources.create(body),
    onSuccess: (resp) => {
      setName("");
      setBaseUrl("");
      setApiKey("");
      setSecretToken("");
      setConfig("{}");
      setFormError(null);
      qc.invalidateQueries({ queryKey: ["sources"] });
      onCreated?.(resp.id);
    },
    onError: (e: unknown) => {
      setFormError(errorMessage(e));
    },
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
      let parsed: JsonObject;
      try {
        parsed = JSON.parse(config || "{}");
        if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) {
          setFormError("Invalid config JSON: expected an object");
          return;
        }
      } catch (e: unknown) {
        setFormError(`Invalid config JSON: ${errorMessage(e)}`);
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
          <code>{window.location.origin}/webhook/{name || "&lt;name&gt;"}</code> with the
          secret token above as the password.
        </p>
      )}
      {formError && (
        <p className="hint" style={{ color: "var(--bad)" }}>
          {formError}
        </p>
      )}
    </div>
  );
}
