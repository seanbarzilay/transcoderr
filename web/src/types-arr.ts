export interface FileSummary {
  path: string;
  size: number;
  codec: string | null;
  quality: string | null;
  resolution: string | null;
}

export interface MovieSummary {
  id: number;
  title: string;
  year: number | null;
  poster_url: string | null;
  has_file: boolean;
  file: FileSummary | null;
}

export interface SeriesSummary {
  id: number;
  title: string;
  year: number | null;
  poster_url: string | null;
  season_count: number;
  episode_count: number;
  episode_file_count: number;
}

export interface SeasonSummary {
  number: number;
  episode_count: number;
  episode_file_count: number;
  monitored: boolean;
}

export interface SeriesDetail {
  id: number;
  title: string;
  year: number | null;
  overview: string | null;
  poster_url: string | null;
  fanart_url: string | null;
  seasons: SeasonSummary[];
}

export interface EpisodeSummary {
  id: number;
  season_number: number;
  episode_number: number;
  title: string;
  air_date: string | null;
  has_file: boolean;
  file: FileSummary | null;
}

export interface MoviesPage {
  items: MovieSummary[];
  total: number;
  page: number;
  limit: number;
  available_codecs: string[];
  available_resolutions: string[];
}

export interface SeriesPage {
  items: SeriesSummary[];
  total: number;
  page: number;
  limit: number;
}

export interface EpisodesPage {
  items: EpisodeSummary[];
  available_codecs: string[];
  available_resolutions: string[];
}

export interface BrowseParams {
  search?: string;
  sort?: "title" | "year";
  page?: number;
  limit?: number;
  codec?: string;
  resolution?: string;
}

export interface EpisodesQuery {
  season?: number;
  codec?: string;
  resolution?: string;
}

export interface TranscodeReq {
  file_path: string;
  title: string;
  movie_id?: number;
  series_id?: number;
  episode_id?: number;
}

export interface TranscodeRunRef {
  flow_id: number;
  flow_name: string;
  run_id: number;
}

export interface TranscodeResp {
  runs: TranscodeRunRef[];
}
