import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

export default function Plugins() {
  const qc = useQueryClient();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      api.plugins.toggle(id, enabled),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["plugins"] }),
  });

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Plugins</h2>
        </div>
      </div>
      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th style={{ width: 120 }}>Version</th>
              <th style={{ width: 140 }}>Kind</th>
              <th style={{ width: 110 }}>Enabled</th>
            </tr>
          </thead>
          <tbody>
            {(plugins.data ?? []).map((p) => (
              <tr key={p.id}>
                <td className="mono">{p.name}</td>
                <td className="dim tnum">{p.version}</td>
                <td>
                  <span className="label">{p.kind}</span>
                </td>
                <td>
                  <input
                    type="checkbox"
                    checked={p.enabled}
                    onChange={(e) =>
                      toggle.mutate({ id: p.id, enabled: e.target.checked })
                    }
                  />
                </td>
              </tr>
            ))}
            {(plugins.data ?? []).length === 0 && !plugins.isLoading && (
              <tr>
                <td colSpan={4} className="empty">
                  No plugins discovered.
                  <div className="hint">
                    Drop a plugin directory into <code>data/plugins/</code> and restart.
                  </div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
