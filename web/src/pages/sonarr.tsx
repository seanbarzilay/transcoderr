import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import SourcePicker from "../components/source-picker";
import PosterGrid from "../components/poster-grid";

export default function Sonarr() {
  const qc = useQueryClient();
  const nav = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const sourceId = searchParams.get("source")
    ? parseInt(searchParams.get("source")!, 10)
    : null;
  const setSourceId = (id: number) => {
    const sp = new URLSearchParams(searchParams);
    sp.set("source", String(id));
    setSearchParams(sp, { replace: true });
  };

  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [sort, setSort] = useState<"title" | "year">("title");
  const [codec, setCodec] = useState("");
  const [resolution, setResolution] = useState("");
  const [page, setPage] = useState(1);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(search), 250);
    return () => clearTimeout(t);
  }, [search]);
  useEffect(() => setPage(1), [debounced, sort, codec, resolution, sourceId]);

  const series = useQuery({
    queryKey: ["arr.series", sourceId, debounced, sort, codec, resolution, page],
    queryFn: () =>
      api.arr.series(sourceId!, {
        search: debounced,
        sort,
        codec: codec || undefined,
        resolution: resolution || undefined,
        page,
        limit: 48,
      }),
    enabled: sourceId != null,
    staleTime: 5 * 60_000,
  });

  return (
    <div className="page">
      <h1>Browse Sonarr</h1>
      <div className="browse-toolbar">
        <SourcePicker kind="sonarr" value={sourceId} onChange={setSourceId} />
        <input
          className="mock-input"
          placeholder="Search series…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ flex: 1, maxWidth: 320 }}
        />
        <select
          className="source-picker"
          value={sort}
          onChange={(e) => setSort(e.target.value as "title" | "year")}
        >
          <option value="title">Sort: title</option>
          <option value="year">Sort: year</option>
        </select>
        <select
          className="source-picker"
          value={codec}
          onChange={(e) => setCodec(e.target.value)}
        >
          <option value="">Codec: any</option>
          {(series.data?.available_codecs ?? []).map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <select
          className="source-picker"
          value={resolution}
          onChange={(e) => setResolution(e.target.value)}
        >
          <option value="">Resolution: any</option>
          {(series.data?.available_resolutions ?? []).map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <button
          type="button"
          className="mock-button"
          onClick={async () => {
            if (sourceId == null) return;
            await api.arr.refresh(sourceId);
            qc.invalidateQueries({ queryKey: ["arr.series", sourceId] });
          }}
          disabled={sourceId == null}
        >
          Refresh
        </button>
      </div>

      {sourceId == null && (
        <div className="empty">Pick a Sonarr source to browse.</div>
      )}
      {series.isError && (
        <div className="empty">
          Couldn't load series: {(series.error as any)?.message ?? "unknown error"}
        </div>
      )}
      {series.isLoading && sourceId != null && (
        <div className="empty">Loading library…</div>
      )}
      {series.data && (
        <>
          <PosterGrid
            items={series.data.items}
            selectedId={null}
            onSelect={(id) => nav(`/sonarr/series/${id}?source=${sourceId}`)}
          />
        </>
      )}
    </div>
  );
}
