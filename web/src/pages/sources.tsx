import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Source } from "../types";
import AddSourceForm from "../components/forms/add-source";

const AUTO_KINDS = ["radarr", "sonarr", "lidarr"] as const;

function isAutoKind(kind: string): boolean {
  return (AUTO_KINDS as readonly string[]).includes(kind);
}

function isAutoSource(src: Source): boolean {
  return isAutoKind(src.kind) && src.config?.arr_notification_id != null;
}

export default function Sources() {
  const qc = useQueryClient();
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });

  const del = useMutation({
    mutationFn: (id: number) => api.sources.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sources"] }),
  });

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Sources</h2>
        </div>
      </div>

      <AddSourceForm />

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
