import { useEffect, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Worker } from "../types";
import AddWorkerForm from "../components/forms/add-worker";
import { PathMappingsModal } from "../components/path-mappings-modal";
import { asRecord } from "../lib/records";

const STALE_AFTER_SECS = 90;

function formatSeen(
  now: number,
  last: number | null,
  enabled: boolean,
): { label: string; status: string } {
  if (!enabled) {
    // Disabled is a UI-only status — last_seen still updates because
    // the local heartbeat fires regardless of enable. The label stays
    // useful as a "yes, the daemon is alive, you just turned it off"
    // hint.
    if (last == null) return { label: "off", status: "disabled" };
    const age = now - last;
    if (age < 60) return { label: `off (${age}s ago)`, status: "disabled" };
    return { label: `off (${Math.floor(age / 60)}m ago)`, status: "disabled" };
  }
  if (last == null) return { label: "never", status: "offline" };
  const age = now - last;
  if (age < STALE_AFTER_SECS) {
    if (age < 60) return { label: `${age}s ago`, status: "connected" };
    return { label: `${Math.floor(age / 60)}m ago`, status: "connected" };
  }
  return { label: `${Math.floor(age / 60)}m ago`, status: "stale" };
}

function hwCapsSummary(caps: unknown): string {
  if (!caps || typeof caps !== "object") return "—";
  const rawDevices = asRecord(caps).devices;
  const devices = Array.isArray(rawDevices) ? rawDevices : [];
  if (devices.length === 0) return "software only";
  const counts: Record<string, number> = {};
  for (const d of devices) {
    const device = asRecord(d);
    const accel = String(device.accel ?? "?").toUpperCase();
    const maxConcurrent =
      typeof device.max_concurrent === "number" ? device.max_concurrent : 1;
    counts[accel] = (counts[accel] ?? 0) + maxConcurrent;
  }
  return Object.entries(counts).map(([a, n]) => `${a} ×${n}`).join(", ");
}

export default function Workers() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["workers"], queryFn: api.workers.list });
  const [addOpen, setAddOpen] = useState(false);
  const [mappingFor, setMappingFor] = useState<{ id: number; name: string; rules: Array<{from: string; to: string}> } | null>(null);
  const [nowTick, setNowTick] = useState<number | null>(null);

  const del = useMutation({
    mutationFn: (id: number) => api.workers.delete(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["workers"] }),
  });

  const togg = useMutation({
    mutationFn: (vars: { id: number; enabled: boolean }) =>
      api.workers.patch(vars.id, { enabled: vars.enabled }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["workers"] }),
  });

  useEffect(() => {
    const id = window.setInterval(() => {
      setNowTick(Math.floor(Date.now() / 1000));
    }, 30_000);
    return () => window.clearInterval(id);
  }, []);

  const now = nowTick ?? Math.floor((list.dataUpdatedAt || 0) / 1000);
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
              <th style={{ width: 90 }}>Enabled</th>
              <th style={{ width: 240 }}></th>
            </tr>
          </thead>
          <tbody>
            {(list.data ?? []).map((w: Worker) => {
              const seen = formatSeen(now, w.last_seen_at, w.enabled);
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
                    <input
                      type="checkbox"
                      checked={w.enabled}
                      disabled={togg.isPending || w.kind === "local"}
                      title={w.kind === "local"
                        ? "the local worker can't be disabled"
                        : undefined}
                      onChange={(e) =>
                        togg.mutate({ id: w.id, enabled: e.target.checked })
                      }
                    />
                  </td>
                  <td>
                    {w.kind === "remote" && (
                      <div style={{ display: "flex", gap: "0.5rem", whiteSpace: "nowrap" }}>
                        <button
                          onClick={() => setMappingFor({
                            id: w.id,
                            name: w.name,
                            rules: w.path_mappings ?? [],
                          })}
                          title="Edit path mappings"
                        >
                          Edit mappings
                        </button>
                        <button
                          className="btn-danger"
                          onClick={() => {
                            if (confirm(`Delete worker "${w.name}"?`)) del.mutate(w.id);
                          }}
                        >
                          Delete
                        </button>
                      </div>
                    )}
                  </td>
                </tr>
              );
            })}
            {(list.data ?? []).length === 0 && !list.isLoading && (
              <tr><td colSpan={7} className="empty">No workers yet.</td></tr>
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

      {mappingFor && (
        <PathMappingsModal
          workerId={mappingFor.id}
          workerName={mappingFor.name}
          initialRules={mappingFor.rules}
          onClose={() => setMappingFor(null)}
          onSaved={() => {
            qc.invalidateQueries({ queryKey: ["workers"] });
          }}
        />
      )}
    </div>
  );
}
