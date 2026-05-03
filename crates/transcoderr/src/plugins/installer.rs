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

struct StagingGuard(Option<PathBuf>);
impl StagingGuard {
    fn disarm(&mut self) { self.0.take(); }
}
impl Drop for StagingGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// Download, verify, extract, atomic-swap. Returns details of the
/// installed plugin on success. The caller is responsible for the
/// post-install bookkeeping (sync_discovered, registry rebuild).
///
/// Optional parameters:
/// - `archive_to`: when `Some`, the verified tarball is moved to that
///   path **before** staging cleanup. Coordinator passes the cache
///   path; worker passes `None`.
/// - `auth_token`: when `Some`, the GET request includes
///   `Authorization: Bearer <token>`. Worker passes its
///   `coordinator_token`; coordinator passes `None` (its catalog
///   fetches don't authenticate to itself).
pub async fn install_from_entry(
    entry: &IndexEntry,
    plugins_dir: &Path,
    archive_to: Option<&Path>,
    auth_token: Option<&str>,
) -> Result<InstalledPlugin, InstallError> {
    std::fs::create_dir_all(plugins_dir)?;
    let suffix: String = (0..8)
        .map(|_| (b'a' + (rand::random::<u8>() % 26)) as char)
        .collect();
    let staging = plugins_dir.join(format!(".tcr-install.{suffix}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut guard = StagingGuard(Some(staging.clone()));

    let tmp_tar = staging.join("plugin.tar.gz");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("reqwest client builds");
    let mut req = client.get(&entry.tarball_url);
    if let Some(token) = auth_token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(InstallError::Layout(format!("HTTP {}", resp.status())));
    }
    let body = resp.bytes().await?;
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let got = hex(&hasher.finalize());
    if got != entry.tarball_sha256.to_lowercase() {
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
        return Err(InstallError::Layout(format!(
            "expected 1 top-level dir, got {}", top_dirs.len()
        )));
    }
    let top_dir = top_dirs.remove(0);
    let top_name = top_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if top_name != entry.name {
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
        return Err(InstallError::Manifest(format!(
            "manifest.name is {:?}, expected {:?}", manifest.name, entry.name
        )));
    }

    // Atomic swap.
    let target = plugins_dir.join(&entry.name);
    let backup = plugins_dir.join(format!(".tcr-old.{}.{suffix}", entry.name));
    let backed_up = if target.exists() {
        std::fs::rename(&target, &backup)?;
        true
    } else {
        false
    };
    if let Err(e) = std::fs::rename(&top_dir, &target) {
        if backed_up {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(InstallError::Io(e));
    }
    let _ = std::fs::remove_dir_all(&backup);

    // Archive the verified tarball if requested. Done after atomic swap
    // so a failed install leaves no partial cache entry.
    if let Some(dest) = archive_to {
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Try rename (same volume); fall back to copy.
        if std::fs::rename(&tmp_tar, dest).is_err() {
            let _ = std::fs::copy(&tmp_tar, dest);
        }
    }

    guard.disarm();
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
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
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
            runtimes: vec![],
            deps: None,
        };
        let installed = install_from_entry(&entry, plugins_dir.path(), None, None).await.unwrap();
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

    #[tokio::test]
    async fn install_fails_on_sha_mismatch_and_leaves_no_staging() {
        let (bytes, _real_sha) = build_tarball("hello", &manifest_for("hello"), true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/hello.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/hello.tar.gz", server.uri()),
            tarball_sha256: "0".repeat(64),  // wrong
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: vec![],
            runtimes: vec![],
            deps: None,
        };
        let err = install_from_entry(&entry, plugins_dir.path(), None, None).await.unwrap_err();
        assert!(matches!(err, InstallError::ShaMismatch { .. }));
        // Plugin dir was not created and staging was cleaned.
        assert!(!plugins_dir.path().join("hello").exists());
        let entries: Vec<_> = std::fs::read_dir(plugins_dir.path()).unwrap()
            .filter_map(|e| e.ok()).collect();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn install_fails_when_top_dir_does_not_match_name() {
        // Tarball top-level dir is "wrong", entry says it's "hello".
        let (bytes, sha) = build_tarball("wrong", &manifest_for("wrong"), true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/x.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/x.tar.gz", server.uri()),
            tarball_sha256: sha,
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: vec![],
            runtimes: vec![],
            deps: None,
        };
        let err = install_from_entry(&entry, plugins_dir.path(), None, None).await.unwrap_err();
        match err {
            InstallError::Layout(msg) => assert!(msg.contains("wrong"), "msg: {msg}"),
            other => panic!("expected Layout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn install_fails_when_manifest_name_does_not_match_entry() {
        // Manifest says name="other" but the entry insists it's "hello".
        // The tarball's top-dir IS "hello" so layout passes -- only the
        // manifest cross-check catches it.
        let bad_manifest = manifest_for("other");
        // Tarball top-dir mismatch would be caught earlier; build a
        // tarball whose top-dir is "hello" but manifest says "other".

        // Build a custom tarball with "hello/" as top-dir + bad_manifest.
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path("hello/").unwrap(); hdr.set_mode(0o755); hdr.set_size(0); hdr.set_cksum();
            tar.append(&hdr, std::io::empty()).unwrap();
            let body = bad_manifest.as_bytes();
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path("hello/manifest.toml").unwrap();
            hdr.set_mode(0o644); hdr.set_size(body.len() as u64); hdr.set_cksum();
            tar.append(&hdr, body).unwrap();
            tar.finish().unwrap();
        }
        let bytes = gz.finish().unwrap();
        let mut h = Sha256::new(); h.update(&bytes);
        let sha = hex(&h.finalize());

        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/x.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/x.tar.gz", server.uri()),
            tarball_sha256: sha,
            homepage: None, min_transcoderr_version: None,
            kind: "subprocess".into(), provides_steps: vec![], runtimes: vec![], deps: None,
        };
        let err = install_from_entry(&entry, plugins_dir.path(), None, None).await.unwrap_err();
        match err {
            InstallError::Manifest(msg) => assert!(msg.contains("other")),
            other => panic!("expected Manifest, got {other:?}"),
        }
        assert!(!plugins_dir.path().join("hello").exists());
    }

    #[tokio::test]
    async fn install_replaces_existing_plugin_dir() {
        // Pre-existing plugins/hello/ has a sentinel file.
        let plugins_dir = tempdir().unwrap();
        let target = plugins_dir.path().join("hello");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("sentinel"), "old").unwrap();

        let (bytes, sha) = build_tarball("hello", &manifest_for("hello"), true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/h.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/h.tar.gz", server.uri()),
            tarball_sha256: sha,
            homepage: None, min_transcoderr_version: None,
            kind: "subprocess".into(), provides_steps: vec![], runtimes: vec![], deps: None,
        };
        install_from_entry(&entry, plugins_dir.path(), None, None).await.unwrap();

        assert!(target.join("manifest.toml").exists());
        assert!(target.join("bin/run").exists());
        assert!(!target.join("sentinel").exists(), "old contents replaced");
    }
}
