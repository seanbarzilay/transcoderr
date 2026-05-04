use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const INLINE_THRESHOLD: usize = 64 * 1024;

pub async fn maybe_spill(
    data_dir: &Path,
    job_id: i64,
    step_id: Option<&str>,
    event_id: i64,
    payload_json: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if payload_json.len() <= INLINE_THRESHOLD {
        return Ok(None);
    }
    let dir = data_dir.join("logs").join(job_id.to_string());
    fs::create_dir_all(&dir).await?;
    let fname = format!("{}-{}.log", step_id.unwrap_or("unknown"), event_id);
    let path = dir.join(&fname);
    let mut f = fs::File::create(&path).await?;
    f.write_all(payload_json.as_bytes()).await?;
    Ok(Some(path))
}
