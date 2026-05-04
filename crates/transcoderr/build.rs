use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const WEB_DIR: &str = "../../web";
const DIST_DIR: &str = "../../web/dist";

fn main() {
    let watched = [
        "src",
        "public",
        "index.html",
        "package.json",
        "package-lock.json",
        "tsconfig.json",
        "tsconfig.app.json",
        "tsconfig.node.json",
        "vite.config.ts",
    ];

    for rel in watched {
        emit_rerun_if_changed(&Path::new(WEB_DIR).join(rel));
    }

    if dist_is_fresh() {
        return;
    }

    if deps_are_stale() {
        run("npm", &["--prefix", WEB_DIR, "ci"]);
    }
    run("npm", &["--prefix", WEB_DIR, "run", "build"]);
}

fn emit_rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    if !path.is_dir() {
        return;
    }

    let entries =
        fs::read_dir(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    for entry in entries {
        let entry = entry
            .unwrap_or_else(|e| panic!("failed to inspect entry under {}: {e}", path.display()));
        emit_rerun_if_changed(&entry.path());
    }
}

fn dist_is_fresh() -> bool {
    let dist = Path::new(DIST_DIR);
    let Some(dist_mtime) = newest_mtime(dist) else {
        return false;
    };

    let sources = [
        PathBuf::from("../../web/src"),
        PathBuf::from("../../web/public"),
        PathBuf::from("../../web/index.html"),
        PathBuf::from("../../web/package.json"),
        PathBuf::from("../../web/package-lock.json"),
        PathBuf::from("../../web/tsconfig.json"),
        PathBuf::from("../../web/tsconfig.app.json"),
        PathBuf::from("../../web/tsconfig.node.json"),
        PathBuf::from("../../web/vite.config.ts"),
    ];

    sources
        .iter()
        .filter_map(|path| newest_mtime(path))
        .all(|mtime| mtime <= dist_mtime)
}

fn deps_are_stale() -> bool {
    let Some(lock_mtime) = newest_mtime(Path::new("../../web/package-lock.json")) else {
        return true;
    };
    let Some(installed_mtime) =
        newest_mtime(Path::new("../../web/node_modules/.package-lock.json"))
    else {
        return true;
    };
    installed_mtime < lock_mtime
}

fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.is_file() {
        return metadata.modified().ok();
    }
    if !metadata.is_dir() {
        return None;
    }

    let mut newest = metadata.modified().ok();
    for entry in fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        let child = newest_mtime(&entry.path())?;
        newest = Some(match newest {
            Some(current) => current.max(child),
            None => child,
        });
    }
    newest
}

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {program}: {e}"));
    assert!(
        status.success(),
        "{program} {} failed with {status}",
        args.join(" ")
    );
}
