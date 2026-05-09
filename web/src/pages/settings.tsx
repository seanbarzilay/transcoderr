import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import ApiTokensCard from "../components/api-tokens-card";

export default function Settings() {
  const qc = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings.get });
  // Auth state lives behind /api/auth/me, not /api/settings. The settings
  // endpoint filters every `auth.*` key server-side (auth.password_hash
  // would otherwise leak the Argon2 hash; auth.enabled is just status
  // that's already on /auth/me as `auth_required`). Read it from the
  // dedicated endpoint so the password-change input still renders.
  const me = useQuery({ queryKey: ["auth-me"], queryFn: api.auth.me });
  const authEnabled = me.data?.auth_required ?? false;
  const [draftOverride, setDraftOverride] = useState<Record<string, string> | null>(null);
  const [password, setPassword] = useState("");
  const draft = draftOverride ?? settings.data ?? {};
  const setDraft = (next: Record<string, string>) => setDraftOverride(next);

  const save = useMutation({
    mutationFn: () => {
      const body: Record<string, string> = { ...draft };
      if (authEnabled && password) {
        body["auth.password"] = password;
        // Re-affirm auth.enabled so the PATCH special-case fires and
        // hashes the new password. The server filters auth.* in the
        // generic loop, so the only way to write the hash is through
        // this branch.
        body["auth.enabled"] = "true";
      }
      return api.settings.patch(body);
    },
    onSuccess: () => {
      setPassword("");
      setDraftOverride(null);
      qc.invalidateQueries({ queryKey: ["settings"] });
      qc.invalidateQueries({ queryKey: ["auth-me"] });
    },
  });

  const keys = Object.keys(draft).sort();

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
                    onChange={(e) => setDraft({ ...draft, [k]: e.target.value })}
                    style={{ width: 360 }}
                  />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {authEnabled && (
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
