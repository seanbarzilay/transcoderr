import { useState } from "react";
import { useParams, useSearchParams, Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import TranscodeButton from "../components/transcode-button";
import { formatBytes } from "../lib/format";
import type { EpisodeSummary } from "../types-arr";

export default function SonarrSeries() {
  const { seriesId } = useParams();
  const [searchParams] = useSearchParams();
  const sourceId = searchParams.get("source")
    ? parseInt(searchParams.get("source")!, 10)
    : null;
  const seriesIdNum = seriesId ? parseInt(seriesId, 10) : null;

  const detail = useQuery({
    queryKey: ["arr.series.get", sourceId, seriesIdNum],
    queryFn: () => api.arr.seriesGet(sourceId!, seriesIdNum!),
    enabled: sourceId != null && seriesIdNum != null,
    staleTime: 5 * 60_000,
  });

  const [activeSeason, setActiveSeason] = useState<number | null>(null);
  const [codec, setCodec] = useState("");
  const [resolution, setResolution] = useState("");
  const seasons = detail.data?.seasons ?? [];
  const defaultSeason = seasons.find((s) => s.number > 0) ?? seasons[0];
  const selectedSeason = activeSeason ?? defaultSeason?.number ?? null;

  const episodes = useQuery({
    queryKey: ["arr.episodes", sourceId, seriesIdNum, selectedSeason, codec, resolution],
    queryFn: () =>
      api.arr.episodes(sourceId!, seriesIdNum!, {
        season: selectedSeason ?? undefined,
        codec: codec || undefined,
        resolution: resolution || undefined,
      }),
    enabled: sourceId != null && seriesIdNum != null && selectedSeason != null,
    staleTime: 5 * 60_000,
  });

  if (sourceId == null) {
    return (
      <div className="page">
        <Link to="/sonarr">← Back to series list</Link>
        <div className="empty">No source selected.</div>
      </div>
    );
  }

  return (
    <div className="page">
      <Link to={`/sonarr?source=${sourceId}`}>← Back to series list</Link>
      {detail.isLoading && <div className="empty">Loading…</div>}
      {detail.data && (
        <>
          <div style={{ display: "flex", gap: 16, marginTop: 12, marginBottom: 16 }}>
            {detail.data.poster_url && (
              <img
                src={detail.data.poster_url}
                alt={detail.data.title}
                style={{ width: 140, borderRadius: 6 }}
              />
            )}
            <div>
              <h1 style={{ margin: 0 }}>{detail.data.title}</h1>
              <div className="muted">{detail.data.year ?? ""}</div>
              {detail.data.overview && (
                <p style={{ marginTop: 8, fontSize: 13, color: "var(--text-dim)" }}>
                  {detail.data.overview}
                </p>
              )}
            </div>
          </div>

          <div className="season-tabs">
            {detail.data.seasons.map((s) => (
              <button
                key={s.number}
                type="button"
                className={
                  "season-tab" + (selectedSeason === s.number ? " is-active" : "")
                }
                onClick={() => setActiveSeason(s.number)}
              >
                {s.number === 0 ? "Specials" : `Season ${s.number}`}
                <span className="muted" style={{ marginLeft: 6, fontSize: 10 }}>
                  {s.episode_file_count}/{s.episode_count}
                </span>
              </button>
            ))}
          </div>

          {episodes.data && (episodes.data.available_codecs.length > 0 || episodes.data.available_resolutions.length > 0) && (
            <div className="browse-toolbar" style={{ marginTop: 0 }}>
              <select
                className="source-picker"
                value={codec}
                onChange={(e) => setCodec(e.target.value)}
              >
                <option value="">Codec: any</option>
                {episodes.data.available_codecs.map((c) => (
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
                {episodes.data.available_resolutions.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
            </div>
          )}

          {episodes.isLoading && <div className="empty">Loading episodes…</div>}
          {episodes.data && (
            <div>
              {episodes.data.items.length === 0 && (
                <div className="empty">No episodes match the current filters.</div>
              )}
              {episodes.data.items.map((ep) => (
                <EpisodeRow
                  key={ep.id}
                  episode={ep}
                  sourceId={sourceId}
                  seriesId={seriesIdNum!}
                  seriesTitle={detail.data!.title}
                />
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function EpisodeRow({
  episode,
  sourceId,
  seriesId,
  seriesTitle,
}: {
  episode: EpisodeSummary;
  sourceId: number;
  seriesId: number;
  seriesTitle: string;
}) {
  return (
    <div className="episode-row">
      <span className="episode-num">
        {String(episode.season_number).padStart(2, "0")}×
        {String(episode.episode_number).padStart(2, "0")}
      </span>
      <div>
        <div className="episode-title">{episode.title}</div>
        {episode.file && (
          <div className="muted" style={{ fontSize: 10, fontFamily: "var(--font-mono)" }}>
            {episode.file.codec ?? ""} · {episode.file.resolution ?? ""} ·{" "}
            {formatBytes(episode.file.size)}
          </div>
        )}
        {!episode.has_file && (
          <div className="hint">no file imported</div>
        )}
      </div>
      <TranscodeButton
        sourceId={sourceId}
        disabled={!episode.has_file}
        disabledReason="no file imported yet"
        payload={{
          file_path: episode.file?.path ?? "",
          title: seriesTitle,
          series_id: seriesId,
          episode_id: episode.id,
        }}
      />
    </div>
  );
}
