import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Source } from "../types";

interface Props {
  kind: "radarr" | "sonarr";
  value: number | null;
  onChange: (sourceId: number) => void;
}

const STORAGE_PREFIX = "transcoderr.last_source.";

function lastSourceKey(kind: string): string {
  return `${STORAGE_PREFIX}${kind}`;
}

export default function SourcePicker({ kind, value, onChange }: Props) {
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });
  const matching = (sources.data ?? []).filter(
    (s: Source) =>
      s.kind === kind && s.config?.arr_notification_id != null,
  );

  // On first load (or when matching list changes), restore last-used or pick first.
  useEffect(() => {
    if (value != null) return;
    if (matching.length === 0) return;
    const remembered = localStorage.getItem(lastSourceKey(kind));
    const rememberedId = remembered ? parseInt(remembered, 10) : NaN;
    const found = matching.find((s) => s.id === rememberedId);
    onChange((found ?? matching[0]).id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [matching.length]);

  if (sources.isLoading) return <div className="muted">loading sources…</div>;
  if (matching.length === 0) {
    return (
      <div className="empty">
        No auto-provisioned {kind} sources.{" "}
        <a href="/sources">Add one on the Sources page.</a>
      </div>
    );
  }

  return (
    <select
      className="source-picker"
      value={value ?? ""}
      onChange={(e) => {
        const id = parseInt(e.target.value, 10);
        localStorage.setItem(lastSourceKey(kind), String(id));
        onChange(id);
      }}
    >
      {matching.map((s) => (
        <option key={s.id} value={s.id}>
          {s.name}
        </option>
      ))}
    </select>
  );
}
