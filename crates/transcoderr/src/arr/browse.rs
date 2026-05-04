//! Schema definitions for the *arr REST endpoints we browse, plus
//! `From` impls that trim them down to the wire-stable summary types
//! in `transcoderr_api_types`. All *arr-schema knowledge lives here so
//! the rest of the codebase doesn't bind to *arr API details.

use serde::Deserialize;
use transcoderr_api_types::{
    EpisodeSummary, FileSummary, MovieSummary, SeasonSummary, SeriesDetail, SeriesSummary,
};

/// Single image entry from an *arr response.
#[derive(Debug, Deserialize)]
pub struct ArrImage {
    #[serde(rename = "coverType", default)]
    pub cover_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "remoteUrl", default)]
    pub remote_url: String,
}

/// Pick `remoteUrl` if non-empty, else fall back to `base_url + url`.
fn pick_image_url(images: &[ArrImage], cover_type: &str, base_url: &str) -> Option<String> {
    let img = images.iter().find(|i| i.cover_type == cover_type)?;
    if !img.remote_url.is_empty() {
        return Some(img.remote_url.clone());
    }
    if !img.url.is_empty() {
        return Some(format!("{}{}", base_url.trim_end_matches('/'), img.url));
    }
    None
}

#[derive(Debug, Deserialize)]
pub struct RadarrMediaInfo {
    #[serde(rename = "videoCodec", default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub resolution: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrQualityName {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrQuality {
    #[serde(default)]
    pub quality: Option<RadarrQualityName>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovieFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "mediaInfo", default)]
    pub media_info: Option<RadarrMediaInfo>,
    #[serde(default)]
    pub quality: Option<RadarrQuality>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovie {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub images: Vec<ArrImage>,
    #[serde(rename = "hasFile", default)]
    pub has_file: bool,
    #[serde(rename = "movieFile", default)]
    pub movie_file: Option<RadarrMovieFile>,
}

impl RadarrMovie {
    pub fn into_summary(self, base_url: &str) -> MovieSummary {
        let poster_url = pick_image_url(&self.images, "poster", base_url);
        let file = if self.has_file {
            self.movie_file.as_ref().map(|f| FileSummary {
                path: f.path.clone(),
                size: f.size,
                codec: f.media_info.as_ref().and_then(|m| m.video_codec.clone()),
                quality: f
                    .quality
                    .as_ref()
                    .and_then(|q| q.quality.as_ref())
                    .and_then(|n| n.name.clone()),
                resolution: f.media_info.as_ref().and_then(|m| m.resolution.clone()),
            })
        } else {
            None
        };
        MovieSummary {
            id: self.id,
            title: self.title,
            year: self.year,
            poster_url,
            has_file: self.has_file,
            file,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeasonStatistics {
    #[serde(rename = "episodeCount", default)]
    pub episode_count: i32,
    #[serde(rename = "episodeFileCount", default)]
    pub episode_file_count: i32,
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeason {
    #[serde(rename = "seasonNumber")]
    pub season_number: i32,
    #[serde(default)]
    pub monitored: bool,
    #[serde(default)]
    pub statistics: Option<SonarrSeasonStatistics>,
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeriesStatistics {
    #[serde(rename = "seasonCount", default)]
    pub season_count: i32,
    #[serde(rename = "episodeCount", default)]
    pub episode_count: i32,
    #[serde(rename = "episodeFileCount", default)]
    pub episode_file_count: i32,
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeries {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub images: Vec<ArrImage>,
    #[serde(default)]
    pub seasons: Vec<SonarrSeason>,
    #[serde(default)]
    pub statistics: Option<SonarrSeriesStatistics>,
}

impl SonarrSeries {
    pub fn into_summary(self, base_url: &str) -> SeriesSummary {
        let stats = self.statistics.as_ref();
        SeriesSummary {
            id: self.id,
            title: self.title,
            year: self.year,
            poster_url: pick_image_url(&self.images, "poster", base_url),
            season_count: stats.map(|s| s.season_count).unwrap_or(0),
            episode_count: stats.map(|s| s.episode_count).unwrap_or(0),
            episode_file_count: stats.map(|s| s.episode_file_count).unwrap_or(0),
            // Populated post-hoc by the series() proxy handler from a
            // concurrent episode fetch — left empty here.
            codecs: Vec::new(),
            resolutions: Vec::new(),
        }
    }

    pub fn into_detail(self, base_url: &str) -> SeriesDetail {
        let poster_url = pick_image_url(&self.images, "poster", base_url);
        let fanart_url = pick_image_url(&self.images, "fanart", base_url);
        let seasons = self
            .seasons
            .iter()
            .map(|s| SeasonSummary {
                number: s.season_number,
                episode_count: s.statistics.as_ref().map(|x| x.episode_count).unwrap_or(0),
                episode_file_count: s
                    .statistics
                    .as_ref()
                    .map(|x| x.episode_file_count)
                    .unwrap_or(0),
                monitored: s.monitored,
            })
            .collect();
        SeriesDetail {
            id: self.id,
            title: self.title,
            year: self.year,
            overview: self.overview,
            poster_url,
            fanart_url,
            seasons,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SonarrEpisodeFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "mediaInfo", default)]
    pub media_info: Option<RadarrMediaInfo>,
    #[serde(default)]
    pub quality: Option<RadarrQuality>,
}

#[derive(Debug, Deserialize)]
pub struct SonarrEpisode {
    pub id: i64,
    #[serde(rename = "seasonNumber")]
    pub season_number: i32,
    #[serde(rename = "episodeNumber")]
    pub episode_number: i32,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "airDate", default)]
    pub air_date: Option<String>,
    #[serde(rename = "hasFile", default)]
    pub has_file: bool,
    #[serde(rename = "episodeFile", default)]
    pub episode_file: Option<SonarrEpisodeFile>,
}

impl SonarrEpisode {
    pub fn into_summary(self) -> EpisodeSummary {
        let file = if self.has_file {
            self.episode_file.as_ref().map(|f| FileSummary {
                path: f.path.clone(),
                size: f.size,
                codec: f.media_info.as_ref().and_then(|m| m.video_codec.clone()),
                quality: f
                    .quality
                    .as_ref()
                    .and_then(|q| q.quality.as_ref())
                    .and_then(|n| n.name.clone()),
                resolution: f.media_info.as_ref().and_then(|m| m.resolution.clone()),
            })
        } else {
            None
        };
        EpisodeSummary {
            id: self.id,
            season_number: self.season_number,
            episode_number: self.episode_number,
            title: self.title,
            air_date: self.air_date,
            has_file: self.has_file,
            file,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn radarr_movie_trims_to_summary_with_file() {
        let raw: RadarrMovie = serde_json::from_value(json!({
            "id": 7,
            "title": "Dune",
            "year": 2021,
            "hasFile": true,
            "images": [
                { "coverType": "poster", "remoteUrl": "https://image.tmdb.org/x.jpg", "url": "/MediaCover/7/poster.jpg" },
                { "coverType": "fanart", "remoteUrl": "https://image.tmdb.org/y.jpg", "url": "" }
            ],
            "movieFile": {
                "path": "/movies/Dune.mkv",
                "size": 42_000_000_000_i64,
                "mediaInfo": { "videoCodec": "x265", "resolution": "3840x2160" },
                "quality": { "quality": { "name": "Bluray-2160p" } }
            }
        })).unwrap();
        let s = raw.into_summary("http://radarr:7878");
        assert_eq!(s.id, 7);
        assert_eq!(s.title, "Dune");
        assert_eq!(
            s.poster_url.as_deref(),
            Some("https://image.tmdb.org/x.jpg")
        );
        assert!(s.has_file);
        let f = s.file.unwrap();
        assert_eq!(f.path, "/movies/Dune.mkv");
        assert_eq!(f.size, 42_000_000_000_i64);
        assert_eq!(f.codec.as_deref(), Some("x265"));
        assert_eq!(f.resolution.as_deref(), Some("3840x2160"));
        assert_eq!(f.quality.as_deref(), Some("Bluray-2160p"));
    }

    #[test]
    fn radarr_movie_no_file_omits_file_summary() {
        let raw: RadarrMovie = serde_json::from_value(json!({
            "id": 8, "title": "Pending", "hasFile": false, "images": []
        }))
        .unwrap();
        let s = raw.into_summary("http://radarr:7878");
        assert!(!s.has_file);
        assert!(s.file.is_none());
    }

    #[test]
    fn radarr_movie_falls_back_to_local_url_when_remote_empty() {
        let raw: RadarrMovie = serde_json::from_value(json!({
            "id": 9, "title": "Local", "hasFile": false,
            "images": [{ "coverType": "poster", "remoteUrl": "", "url": "/MediaCover/9/poster.jpg" }]
        })).unwrap();
        let s = raw.into_summary("http://radarr:7878/");
        assert_eq!(
            s.poster_url.as_deref(),
            Some("http://radarr:7878/MediaCover/9/poster.jpg")
        );
    }

    #[test]
    fn sonarr_series_trims_to_summary() {
        let raw: SonarrSeries = serde_json::from_value(json!({
            "id": 1, "title": "Foundation", "year": 2021,
            "images": [{ "coverType": "poster", "remoteUrl": "https://artworks.thetvdb.com/p.jpg" }],
            "statistics": { "seasonCount": 3, "episodeCount": 30, "episodeFileCount": 25 }
        })).unwrap();
        let s = raw.into_summary("http://sonarr:8989");
        assert_eq!(s.id, 1);
        assert_eq!(s.season_count, 3);
        assert_eq!(s.episode_count, 30);
        assert_eq!(s.episode_file_count, 25);
        assert_eq!(
            s.poster_url.as_deref(),
            Some("https://artworks.thetvdb.com/p.jpg")
        );
    }

    #[test]
    fn sonarr_series_into_detail_carries_seasons_and_fanart() {
        let raw: SonarrSeries = serde_json::from_value(json!({
            "id": 1, "title": "Foundation",
            "overview": "Galactic decline",
            "images": [
                { "coverType": "poster", "remoteUrl": "https://p" },
                { "coverType": "fanart", "remoteUrl": "https://f" }
            ],
            "seasons": [
                { "seasonNumber": 1, "monitored": true,
                  "statistics": { "episodeCount": 10, "episodeFileCount": 10 } },
                { "seasonNumber": 2, "monitored": false,
                  "statistics": { "episodeCount": 10, "episodeFileCount": 5 } }
            ]
        }))
        .unwrap();
        let d = raw.into_detail("http://sonarr:8989");
        assert_eq!(d.fanart_url.as_deref(), Some("https://f"));
        assert_eq!(d.seasons.len(), 2);
        assert_eq!(d.seasons[1].number, 2);
        assert!(!d.seasons[1].monitored);
        assert_eq!(d.seasons[1].episode_file_count, 5);
    }

    #[test]
    fn sonarr_episode_trims_to_summary() {
        let raw: SonarrEpisode = serde_json::from_value(json!({
            "id": 100, "seasonNumber": 1, "episodeNumber": 3,
            "title": "Pilot", "airDate": "2021-09-24", "hasFile": true,
            "episodeFile": {
                "path": "/tv/Foundation/S01E03.mkv", "size": 5_000_000_000_i64,
                "mediaInfo": { "videoCodec": "h264", "resolution": "1920x1080" },
                "quality": { "quality": { "name": "WEBDL-1080p" } }
            }
        }))
        .unwrap();
        let s = raw.into_summary();
        assert_eq!(s.id, 100);
        assert_eq!(s.season_number, 1);
        assert_eq!(s.episode_number, 3);
        assert_eq!(s.title, "Pilot");
        assert_eq!(s.air_date.as_deref(), Some("2021-09-24"));
        let f = s.file.unwrap();
        assert_eq!(f.codec.as_deref(), Some("h264"));
        assert_eq!(f.resolution.as_deref(), Some("1920x1080"));
        assert_eq!(f.quality.as_deref(), Some("WEBDL-1080p"));
    }
}
