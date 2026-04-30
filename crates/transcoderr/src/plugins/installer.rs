use crate::plugins::catalog::IndexEntry;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("download failed: {0}")]
    Download(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sha mismatch: expected {expected}, got {got}")]
    ShaMismatch { expected: String, got: String },
    #[error("tarball layout: {0}")]
    Layout(String),
    #[error("manifest: {0}")]
    Manifest(String),
}

#[derive(Debug)]
pub struct InstalledPlugin {
    pub name: String,
    pub plugin_dir: PathBuf,
    pub tarball_sha256: String,
}

/// Download, verify, extract, atomic-swap. Returns details of the
/// installed plugin on success. The caller is responsible for the
/// post-install bookkeeping (sync_discovered, registry rebuild).
pub async fn install_from_entry(
    entry: &IndexEntry,
    plugins_dir: &Path,
) -> Result<InstalledPlugin, InstallError> {
    std::fs::create_dir_all(plugins_dir)?;
    let suffix: String = (0..8)
        .map(|_| (b'a' + (rand::random::<u8>() % 26)) as char)
        .collect();
    let staging = plugins_dir.join(format!(".tcr-install.{suffix}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;

    // Stream-download into the staging dir as a temp file, hashing as we go.
    let tmp_tar = staging.join("plugin.tar.gz");
    let resp = reqwest::Client::new().get(&entry.tarball_url).send().await?;
    if !resp.status().is_success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Layout(format!("HTTP {}", resp.status())));
    }
    let body = resp.bytes().await?;
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let got = hex(&hasher.finalize());
    if got != entry.tarball_sha256.to_lowercase() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::ShaMismatch {
            expected: entry.tarball_sha256.clone(),
            got,
        });
    }
    let mut f = std::fs::File::create(&tmp_tar)?;
    f.write_all(&body)?;
    drop(f);

    // Untar into staging/extracted/.
    let extracted = staging.join("extracted");
    std::fs::create_dir_all(&extracted)?;
    let f = std::fs::File::open(&tmp_tar)?;
    let gz = GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(&extracted)?;

    // Verify exactly one top-level dir matching entry.name.
    let mut top_dirs: Vec<PathBuf> = std::fs::read_dir(&extracted)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().is_some_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    if top_dirs.len() != 1 {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Layout(format!(
            "expected 1 top-level dir, got {}", top_dirs.len()
        )));
    }
    let top_dir = top_dirs.remove(0);
    let top_name = top_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if top_name != entry.name {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Layout(format!(
            "top-level dir is {top_name:?}, expected {:?}", entry.name
        )));
    }

    // Verify manifest parses and name matches.
    let manifest_raw = std::fs::read_to_string(top_dir.join("manifest.toml"))
        .map_err(|e| InstallError::Manifest(format!("manifest.toml: {e}")))?;
    let manifest: crate::plugins::manifest::Manifest = toml::from_str(&manifest_raw)
        .map_err(|e| InstallError::Manifest(format!("parse: {e}")))?;
    if manifest.name != entry.name {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Manifest(format!(
            "manifest.name is {:?}, expected {:?}", manifest.name, entry.name
        )));
    }

    // Atomic swap.
    let target = plugins_dir.join(&entry.name);
    let backup = plugins_dir.join(format!(".tcr-old.{}.{suffix}", entry.name));
    if target.exists() {
        std::fs::rename(&target, &backup)?;
    }
    std::fs::rename(&top_dir, &target)?;
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&staging);

    Ok(InstalledPlugin {
        name: entry.name.clone(),
        plugin_dir: target,
        tarball_sha256: got,
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { let _ = write!(s, "{:02x}", b); }
    s
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use serde_json::json;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a tar.gz of `<plugin_name>/manifest.toml` (+ optional bin/run)
    /// in memory and return (bytes, sha256_hex).
    fn build_tarball(plugin_name: &str, manifest_toml: &str, with_bin_run: bool) -> (Vec<u8>, String) {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);

            // Top-level dir entry.
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path(format!("{plugin_name}/")).unwrap();
            hdr.set_mode(0o755);
            hdr.set_size(0);
            hdr.set_cksum();
            tar.append(&hdr, std::io::empty()).unwrap();

            // manifest.toml.
            let manifest = manifest_toml.as_bytes();
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path(format!("{plugin_name}/manifest.toml")).unwrap();
            hdr.set_mode(0o644);
            hdr.set_size(manifest.len() as u64);
            hdr.set_cksum();
            tar.append(&hdr, manifest).unwrap();

            if with_bin_run {
                let body = b"#!/bin/sh\necho ok\n";
                let mut hdr = tar::Header::new_gnu();
                hdr.set_path(format!("{plugin_name}/bin/run")).unwrap();
                hdr.set_mode(0o755);
                hdr.set_size(body.len() as u64);
                hdr.set_cksum();
                tar.append(&hdr, &body[..]).unwrap();
            }

            tar.finish().unwrap();
        }
        let bytes = gz.finish().unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        let sha = hex(&h.finalize());
        (bytes, sha)
    }

    fn manifest_for(name: &str) -> String {
        format!(
            r#"name = "{name}"
version = "0.1.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["{name}.do"]
"#
        )
    }

    #[tokio::test]
    async fn install_happy_path_extracts_and_swaps() {
        let (bytes, sha) = build_tarball("hello", &manifest_for("hello"), true);

        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/hello.tar.gz"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_bytes(bytes)
                .insert_header("content-type", "application/gzip"))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/hello.tar.gz", server.uri()),
            tarball_sha256: sha.clone(),
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: vec!["hello.do".into()],
        };
        let installed = install_from_entry(&entry, plugins_dir.path()).await.unwrap();
        assert_eq!(installed.name, "hello");
        assert_eq!(installed.tarball_sha256, sha);
        assert!(installed.plugin_dir.exists());
        assert!(installed.plugin_dir.join("manifest.toml").exists());
        assert!(installed.plugin_dir.join("bin/run").exists());
        // No leftover staging dir.
        let leftovers: Vec<_> = std::fs::read_dir(plugins_dir.path()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tcr-"))
            .collect();
        assert!(leftovers.is_empty(), "staging dirs should be cleaned up");
    }
}
