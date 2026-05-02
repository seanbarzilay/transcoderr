import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Worker } from "../types";
import AddWorkerForm from "../components/forms/add-worker";

const STALE_AFTER_SECS = 90;

function formatSeen(now: number, last: number | null): { label: string; status: string } {
  if (last == null) return { label: "never", status: "offline" };
  const age = now - last;
  if (age < STALE_AFTER_SECS) {
    if (age < 60) return { label: `${age}s ago`, status: "connected" };
    return { label: `${Math.floor(age / 60)}m ago`, status: "connected" };
  }
  return { label: `${Math.floor(age / 60)}m ago`, status: "stale" };
}

function hwCapsSummary(caps: any): string {
  if (!caps || typeof caps !== "object") return "—";
  const devices = Array.isArray(caps.devices) ? caps.devices : [];
  if (devices.length === 0) return "software only";
  const counts: Record<string, number> = {};
  for (const d of devices) {
    const accel = String(d.accel ?? "?").toUpperCase();
    counts[accel] = (counts[accel] ?? 0) + (d.max_concurrent ?? 1);
  }
  return Object.entries(counts).map(([a, n]) => `${a} ×${n}`).join(", ");
}

export default function Workers() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["workers"], queryFn: api.workers.list });
  const [addOpen, setAddOpen] = useState(false);

  const del = useMutation({
    mutationFn: (id: number) => api.workers.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["workers"] }),
  });

  const now = Math.floor(Date.now() / 1000);
  const coordinatorUrlGuess = window.location.origin
    .replace(/^http:/, "ws:")
    .replace(/^https:/, "wss:");

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Workers</h2>
        </div>
        <button onClick={() => setAddOpen(true)}>Add worker</button>
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th style={{ width: 110 }}>Status</th>
              <th>Name</th>
              <th style={{ width: 90 }}>Kind</th>
              <th>Hardware</th>
              <th style={{ width: 130 }}>Last seen</th>
              <th style={{ width: 90 }}></th>
            </tr>
          </thead>
          <tbody>
            {(list.data ?? []).map((w: Worker) => {
              const seen = formatSeen(now, w.last_seen_at);
              return (
                <tr key={w.id}>
                  <td>
                    <span className={`badge badge-${seen.status}`}>{seen.status}</span>
                  </td>
                  <td>{w.name}</td>
                  <td><span className="label">{w.kind}</span></td>
                  <td className="mono dim">{hwCapsSummary(w.hw_caps)}</td>
                  <td className="dim">{seen.label}</td>
                  <td>
                    {w.kind === "remote" && (
                      <button
                        className="btn-danger"
                        onClick={() => {
                          if (confirm(`Delete worker "${w.name}"?`)) del.mutate(w.id);
                        }}
                      >
                        Delete
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
            {(list.data ?? []).length === 0 && !list.isLoading && (
              <tr><td colSpan={6} className="empty">No workers yet.</td></tr>
            )}
          </tbody>
        </table>
      </div>

      {addOpen && (
        <AddWorkerForm
          coordinatorUrlGuess={coordinatorUrlGuess}
          onClose={() => setAddOpen(false)}
        />
      )}
    </div>
  );
}
