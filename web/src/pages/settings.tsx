import { useState, useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

export default function Settings() {
  const qc = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.settings.get });
  const [draft, setDraft] = useState<Record<string,string>>({});
  const [password, setPassword] = useState("");

  useEffect(() => { if (settings.data) setDraft(settings.data); }, [settings.data]);

  const save = useMutation({
    mutationFn: () => {
      const body: Record<string,string> = { ...draft };
      if (draft["auth.enabled"] === "true" && password) body["auth.password"] = password;
      return api.settings.patch(body);
    },
    onSuccess: () => { setPassword(""); qc.invalidateQueries({ queryKey: ["settings"] }); },
  });

  return (
    <div style={{ padding: 24 }}>
      <h2>Settings</h2>
      <table>
        <thead><tr><th>Key</th><th>Value</th></tr></thead>
        <tbody>
          {Object.keys(draft).filter(k => k !== "auth.password_hash").map(k => (
            <tr key={k}>
              <td><code>{k}</code></td>
              <td>
                <input
                  value={draft[k] ?? ""}
                  onChange={e => setDraft(d => ({ ...d, [k]: e.target.value }))}
                  style={{ width: 320 }}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {draft["auth.enabled"] === "true" && (
        <div style={{ marginTop: 12 }}>
          <input type="password" placeholder="New password (leave blank to keep current)" value={password}
            onChange={e => setPassword(e.target.value)} style={{ width: 320 }} />
        </div>
      )}
      <button onClick={() => save.mutate()} style={{ marginTop: 12 }}>Save</button>
    </div>
  );
}
