fn main() {
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
    if std::path::Path::new("web/dist").exists() { return; }
    let _ = std::process::Command::new("npm").args(["--prefix", "web", "ci"]).status();
    let _ = std::process::Command::new("npm").args(["--prefix", "web", "run", "build"]).status();
}
