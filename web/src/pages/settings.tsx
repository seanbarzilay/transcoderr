import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import ApiTokensCard from "../components/api-tokens-card";

export default function Settings() {
  const qc = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings.get });
  const [draftOverride, setDraftOverride] = useState<Record<string, string> | null>(null);
  const [dirty, setDirty] = useState<Set<string>>(new Set());
  const [password, setPassword] = useState("");
  const draft = draftOverride ?? settings.data ?? {};
  const setDraft = (key: string, value: string) => {
    setDraftOverride({ ...draft, [key]: value });
    setDirty((prev) => new Set(prev).add(key));
  };

  const save = useMutation({
    mutationFn: () => {
      // Send only what was actually edited. PATCHing the whole settings
      // map back is what used to write a stale auth.password_hash over a
      // freshly rotated one, and it makes every save depend on keys this
      // page never touched.
      const body: Record<string, string> = {};
      for (const k of dirty) body[k] = draft[k] ?? "";
      if (password) {
        body["auth.password"] = password;
        // The server only (re)hashes a password as part of the
        // enable-auth branch, so this key has to travel with it.
        body["auth.enabled"] = draft["auth.enabled"] ?? "true";
      }
      return api.settings.patch(body);
    },
    onSuccess: () => {
      setPassword("");
      setDraftOverride(null);
      setDirty(new Set());
      qc.invalidateQueries({ queryKey: ["settings"] });
    },
  });

  // `auth.password_hash` is no longer sent by the server; the filter
  // stays so an older coordinator doesn't render it into an input.
  const keys = Object.keys(draft)
    .filter((k) => k !== "auth.password_hash")
    .sort();

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Settings</h2>
        </div>
        <button onClick={() => save.mutate()} disabled={save.isPending}>
          Save
        </button>
      </div>

      {save.isError && (
        <div style={{ color: "#f88", fontSize: 12, marginTop: 6 }}>
          {(save.error as Error)?.message ?? "save failed"}
        </div>
      )}

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Key</th>
              <th>Value</th>
            </tr>
          </thead>
          <tbody>
            {keys.map((k) => (
              <tr key={k}>
                <td className="mono dim">{k}</td>
                <td>
                  <input
                    value={draft[k] ?? ""}
                    onChange={(e) => setDraft(k, e.target.value)}
                    style={{ width: 360 }}
                  />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {draft["auth.enabled"] === "true" && (
        <div className="surface" style={{ padding: 16, marginTop: 16 }}>
          <div className="label" style={{ marginBottom: 6 }}>
            New password
          </div>
          <input
            type="password"
            placeholder="leave blank to keep current"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            style={{ width: 360 }}
          />
        </div>
      )}

      <ApiTokensCard />
    </div>
  );
}
