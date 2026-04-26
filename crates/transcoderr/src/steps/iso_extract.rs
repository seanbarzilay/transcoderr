//! `iso.extract` step: detects Blu-ray ISO inputs, demuxes the largest
//! `BDMV/STREAM/*.m2ts`, and threads it through the staging chain.

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub is_folder: bool,
}

/// Parse the output of `7z l -slt <iso>` into entries.
/// Skips the archive-level header (the first block before the first
/// `----------` separator) and yields one entry per file/folder block.
pub fn parse_7z_listing(stdout: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for chunk in stdout.split("----------").skip(1) {
        let mut path: Option<String> = None;
        let mut size: Option<u64> = None;
        let mut is_folder = false;
        for line in chunk.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("Path = ") {
                path = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("Size = ") {
                size = rest.parse().ok();
            } else if let Some(rest) = line.strip_prefix("Folder = ") {
                is_folder = rest == "+";
            }
        }
        if let (Some(p), Some(s)) = (path, size) {
            out.push(Entry { path: p, size: s, is_folder });
        }
    }
    out
}

/// Pick the largest non-folder `BDMV/STREAM/*.m2ts` entry. Case-insensitive.
pub fn pick_largest_m2ts(entries: &[Entry]) -> Option<&Entry> {
    entries
        .iter()
        .filter(|e| !e.is_folder)
        .filter(|e| {
            let lower = e.path.to_lowercase();
            lower.starts_with("bdmv/stream/") && lower.ends_with(".m2ts")
        })
        .max_by_key(|e| e.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/7z_listing_bdmv.txt");

    #[test]
    fn parses_entries_from_real_listing() {
        let entries = parse_7z_listing(FIXTURE);
        // 6 entries: index.bdmv, STREAM (folder), 3x m2ts, ID.BDMV
        assert_eq!(entries.len(), 6, "got {entries:#?}");
        assert!(entries.iter().any(|e| e.path == "BDMV/index.bdmv" && e.size == 110 && !e.is_folder));
        assert!(entries.iter().any(|e| e.path == "BDMV/STREAM" && e.is_folder));
        assert!(entries.iter().any(|e| e.path == "BDMV/STREAM/00001.m2ts" && e.size == 36507942912));
    }

    #[test]
    fn picks_largest_m2ts() {
        let entries = parse_7z_listing(FIXTURE);
        let pick = pick_largest_m2ts(&entries).expect("should find one");
        assert_eq!(pick.path, "BDMV/STREAM/00001.m2ts");
        assert_eq!(pick.size, 36507942912);
    }

    #[test]
    fn picks_none_when_no_bdmv_streams() {
        let entries = vec![
            Entry { path: "VIDEO_TS/VTS_01_1.VOB".into(), size: 1024, is_folder: false },
            Entry { path: "BDMV/STREAM".into(), size: 0, is_folder: true },
        ];
        assert!(pick_largest_m2ts(&entries).is_none());
    }

    #[test]
    fn ignores_folder_entries_with_matching_path() {
        let entries = vec![
            Entry { path: "BDMV/STREAM/junk".into(), size: 999_999_999, is_folder: true },
            Entry { path: "BDMV/STREAM/00001.m2ts".into(), size: 1024, is_folder: false },
        ];
        let pick = pick_largest_m2ts(&entries).unwrap();
        assert_eq!(pick.path, "BDMV/STREAM/00001.m2ts");
    }

    #[test]
    fn case_insensitive_match() {
        let entries = vec![
            Entry { path: "bdmv/stream/00001.M2TS".into(), size: 1024, is_folder: false },
        ];
        let pick = pick_largest_m2ts(&entries).unwrap();
        assert_eq!(pick.size, 1024);
    }
}
