import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

export default function Plugins() {
  const qc = useQueryClient();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) => api.plugins.toggle(id, enabled),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["plugins"] }),
  });

  return (
    <div style={{ padding: 24 }}>
      <h2>Plugins</h2>
      <table>
        <thead><tr><th>Name</th><th>Version</th><th>Kind</th><th>Enabled</th></tr></thead>
        <tbody>
          {(plugins.data ?? []).map(p => (
            <tr key={p.id}>
              <td>{p.name}</td>
              <td>{p.version}</td>
              <td>{p.kind}</td>
              <td>
                <input
                  type="checkbox"
                  checked={p.enabled}
                  onChange={e => toggle.mutate({ id: p.id, enabled: e.target.checked })}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
