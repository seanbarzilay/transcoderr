import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import SourcePicker from "../components/source-picker";
import PosterGrid from "../components/poster-grid";
import DetailPanel, { FileSummaryRow } from "../components/detail-panel";
import TranscodeButton from "../components/transcode-button";
import { errorMessage } from "../lib/errors";
import { formatBytes } from "../lib/format";
import type { MovieSummary } from "../types-arr";

export default function Radarr() {
  const qc = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const sourceId = searchParams.get("source")
    ? parseInt(searchParams.get("source")!, 10)
    : null;
  const setSourceId = (id: number) => {
    const sp = new URLSearchParams(searchParams);
    sp.set("source", String(id));
    setSearchParams(sp, { replace: true });
    setPage(1);
  };

  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [sort, setSort] = useState<"title" | "year">("title");
  const [codec, setCodec] = useState("");
  const [resolution, setResolution] = useState("");
  const [page, setPage] = useState(1);
  const [selectedId, setSelectedId] = useState<number | null>(null);

  useEffect(() => {
    const t = setTimeout(() => {
      setDebounced(search);
      setPage(1);
    }, 250);
    return () => clearTimeout(t);
  }, [search]);

  const movies = useQuery({
    queryKey: ["arr.movies", sourceId, debounced, sort, codec, resolution, page],
    queryFn: () =>
      api.arr.movies(sourceId!, {
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

  const selected = movies.data?.items.find((m) => m.id === selectedId) ?? null;

  return (
    <div className="page">
      <h1>Browse Radarr</h1>
      <div className="browse-toolbar">
        <SourcePicker kind="radarr" value={sourceId} onChange={setSourceId} />
        <input
          className="mock-input"
          placeholder="Search movies…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ flex: 1, maxWidth: 320 }}
        />
        <select
          className="source-picker"
          value={sort}
          onChange={(e) => {
            setSort(e.target.value as "title" | "year");
            setPage(1);
          }}
        >
          <option value="title">Sort: title</option>
          <option value="year">Sort: year</option>
        </select>
        <select
          className="source-picker"
          value={codec}
          onChange={(e) => {
            setCodec(e.target.value);
            setPage(1);
          }}
        >
          <option value="">Codec: any</option>
          {(movies.data?.available_codecs ?? []).map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <select
          className="source-picker"
          value={resolution}
          onChange={(e) => {
            setResolution(e.target.value);
            setPage(1);
          }}
        >
          <option value="">Resolution: any</option>
          {(movies.data?.available_resolutions ?? []).map((r) => (
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
            qc.invalidateQueries({ queryKey: ["arr.movies", sourceId] });
          }}
          disabled={sourceId == null}
        >
          Refresh
        </button>
      </div>

      {sourceId == null && (
        <div className="empty">Pick a Radarr source to browse.</div>
      )}
      {movies.isError && (
        <div className="empty">
          Couldn't load movies: {errorMessage(movies.error, "unknown error")}
        </div>
      )}
      {movies.isLoading && sourceId != null && (
        <div className="empty">Loading library…</div>
      )}
      {movies.data && (
        <>
          <PosterGrid
            items={movies.data.items}
            onSelect={setSelectedId}
            selectedId={selectedId}
          />
          <Pager
            page={movies.data.page}
            limit={movies.data.limit}
            total={movies.data.total}
            onChange={setPage}
          />
        </>
      )}

      <DetailPanel open={selected != null} onClose={() => setSelectedId(null)}>
        {selected && (
          <MovieDetail
            movie={selected}
            sourceId={sourceId!}
          />
        )}
      </DetailPanel>
    </div>
  );
}

function MovieDetail({
  movie,
  sourceId,
}: {
  movie: MovieSummary;
  sourceId: number;
}) {
  return (
    <>
      {movie.poster_url && (
        <img
          src={movie.poster_url}
          alt={movie.title}
          style={{ width: "100%", borderRadius: 6, marginBottom: 12 }}
        />
      )}
      <h2 style={{ margin: 0 }}>{movie.title}</h2>
      <div className="muted" style={{ marginBottom: 12 }}>
        {movie.year ?? ""}
      </div>
      {movie.file ? (
        <>
          <FileSummaryRow label="Path" value={movie.file.path} />
          <FileSummaryRow label="Size" value={formatBytes(movie.file.size)} />
          <FileSummaryRow label="Codec" value={movie.file.codec} />
          <FileSummaryRow label="Resolution" value={movie.file.resolution} />
          <FileSummaryRow label="Quality" value={movie.file.quality} />
        </>
      ) : (
        <div className="hint">No file imported yet.</div>
      )}
      <TranscodeButton
        sourceId={sourceId}
        disabled={!movie.has_file}
        disabledReason="no file imported yet"
        payload={{
          file_path: movie.file?.path ?? "",
          title: movie.title,
          movie_id: movie.id,
        }}
      />
    </>
  );
}

function Pager({
  page,
  limit,
  total,
  onChange,
}: {
  page: number;
  limit: number;
  total: number;
  onChange: (p: number) => void;
}) {
  const lastPage = Math.max(1, Math.ceil(total / limit));
  if (lastPage <= 1) return null;
  return (
    <div style={{ display: "flex", gap: 8, marginTop: 16, alignItems: "center" }}>
      <button
        type="button"
        className="mock-button"
        onClick={() => onChange(page - 1)}
        disabled={page <= 1}
      >
        ←
      </button>
      <span className="muted">
        Page {page} of {lastPage} ({total} total)
      </span>
      <button
        type="button"
        className="mock-button"
        onClick={() => onChange(page + 1)}
        disabled={page >= lastPage}
      >
        →
      </button>
    </div>
  );
}
