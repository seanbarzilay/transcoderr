//! End-to-end: spin up `transcoderr serve` on an ephemeral port with a
//! tempdir DB, seed an api_token row, drive `transcoderr-mcp` over stdio,
//! exercise the happy path: list_runs → create_flow → dry_run_flow.

use serial_test::serial;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FLOW_YAML: &str = r#"name: e2e-flow
triggers:
  - radarr: [downloaded]
steps:
  - use: probe
"#;

fn wait_until_healthy(url: &str, deadline: Duration) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if reqwest::blocking::get(format!("{url}/healthz"))
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("server did not become healthy within {deadline:?}");
}

fn jsonrpc(line: &str) -> serde_json::Value {
    serde_json::from_str(line).expect("valid jsonrpc")
}

#[test]
#[serial]
fn mcp_stdio_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // Write a minimal config.toml.
    let cfg_path = data_dir.join("config.toml");
    std::fs::write(
        &cfg_path,
        format!(
            r#"
bind = "127.0.0.1:0"
data_dir = "{}"
[radarr]
bearer_token = "test"
"#,
            data_dir.display()
        ),
    )
    .unwrap();

    // Start `transcoderr serve` on an ephemeral port. Capture stderr to find the bound port.
    // CARGO_BIN_EXE_<name> is only set for the same package's bins, so derive the sibling
    // `transcoderr` binary path from the mcp binary's directory.
    let mcp_bin = env!("CARGO_BIN_EXE_transcoderr-mcp");
    let bin_dir = std::path::Path::new(mcp_bin)
        .parent()
        .expect("mcp bin parent");
    let server_bin = bin_dir.join(if cfg!(windows) {
        "transcoderr.exe"
    } else {
        "transcoderr"
    });
    assert!(
        server_bin.exists(),
        "transcoderr binary not found at {server_bin:?}; run `cargo build -p transcoderr` first"
    );
    let mut server = Command::new(&server_bin)
        .arg("serve")
        .arg("--config")
        .arg(&cfg_path)
        .env("RUST_LOG", "warn,transcoderr=info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");

    // Read stderr until we see the actual bound addr ("127.0.0.1:NNNNN").
    // The server logs `addr=<actual_local_addr>` after binding.
    let stderr = server.stderr.take().unwrap();
    let mut rdr = BufReader::new(stderr);
    let mut port: Option<u16> = None;
    for _ in 0..200 {
        let mut line = String::new();
        if rdr.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if let Some(idx) = line.find("127.0.0.1:") {
            let rest = &line[idx + "127.0.0.1:".len()..];
            let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = n.parse::<u16>() {
                if p != 0 {
                    port = Some(p);
                    break;
                }
            }
        }
    }
    let port = port.expect("did not parse bound port from server stderr");
    let url = format!("http://127.0.0.1:{port}");
    wait_until_healthy(&url, Duration::from_secs(5));

    // Seed an api_token row directly.
    let db_path = data_dir.join("data.db");
    let raw_token = "tcr_E2ETESTTOKEN1234567890ABCDEFGHIJKL"; // tcr_ + 32 chars
    seed_api_token(&db_path, raw_token);

    // Spawn transcoderr-mcp.
    let mut mcp = Command::new(mcp_bin)
        .env("TRANSCODERR_URL", &url)
        .env("TRANSCODERR_TOKEN", raw_token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp");

    let mut stdin = mcp.stdin.take().unwrap();
    let stdout = mcp.stdout.take().unwrap();
    let mut stdout = BufReader::new(stdout);

    fn send(stdin: &mut impl Write, v: serde_json::Value) {
        let s = serde_json::to_string(&v).unwrap();
        writeln!(stdin, "{s}").unwrap();
        stdin.flush().unwrap();
    }
    fn recv(stdout: &mut impl BufRead) -> serde_json::Value {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        jsonrpc(&line)
    }

    // initialize
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}
        }),
    );
    let init = recv(&mut stdout);
    assert!(
        init["result"]["serverInfo"]["name"].as_str().is_some(),
        "init result: {init}"
    );

    // initialized notification
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0","method":"notifications/initialized","params":{}
        }),
    );

    // tools/list
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/list","params":{}
        }),
    );
    let listed = recv(&mut stdout);
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|t| t["name"] == "list_runs"),
        "tools/list missing list_runs: {listed}"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "create_flow"),
        "tools/list missing create_flow"
    );
    // Plugin tools surface but catalog management does not -- catalogs
    // are infrastructure config (like notifier secrets), plugins are
    // operational state the AI client can act on.
    assert!(
        tools.iter().any(|t| t["name"] == "list_plugins"),
        "tools/list missing list_plugins"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "browse_plugins"),
        "tools/list missing browse_plugins"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "install_plugin"),
        "tools/list missing install_plugin"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "uninstall_plugin"),
        "tools/list missing uninstall_plugin"
    );
    assert!(
        !tools
            .iter()
            .any(|t| t["name"].as_str().is_some_and(|n| n.contains("catalog"))),
        "catalog management tools should not surface to MCP: {listed}"
    );

    // call list_runs (empty list initially)
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"list_runs","arguments":{}}
        }),
    );
    let runs = recv(&mut stdout);
    assert!(
        runs.get("error").is_none() || runs["error"].is_null(),
        "list_runs failed: {runs}"
    );

    // call create_flow
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"create_flow","arguments":{"name":"e2e","yaml":FLOW_YAML}}
        }),
    );
    let created = recv(&mut stdout);
    // rmcp 0.3.2's Json<T> wrapper serializes the response as a text content
    // block whose `text` is the JSON-encoded T. Extract and parse to get the id.
    let txt = created["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("create_flow missing content[0].text: {created}"));
    let payload: serde_json::Value =
        serde_json::from_str(txt).unwrap_or_else(|_| panic!("create_flow content not JSON: {txt}"));
    assert!(
        payload["id"].as_i64().is_some(),
        "create_flow payload missing id: {payload}"
    );
    assert_eq!(payload["name"], "e2e");

    // call dry_run_flow
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"dry_run_flow","arguments":{"yaml":FLOW_YAML,"file_path":"/x.mkv"}}
        }),
    );
    let dry = recv(&mut stdout);
    assert!(
        dry.get("error").is_none() || dry["error"].is_null(),
        "dry_run_flow failed: {dry}"
    );

    // shutdown
    drop(stdin);
    let _ = wait_or_kill(&mut mcp, Duration::from_secs(2));
    let _ = server.kill();
    let _ = server.wait();
}

fn seed_api_token(db_path: &std::path::Path, raw_token: &str) {
    use sqlx::sqlite::SqlitePoolOptions;
    let path = db_path.to_path_buf();
    let raw = raw_token.to_string();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let url = format!("sqlite://{}", path.display());
        let pool = SqlitePoolOptions::new()
            .connect(&url)
            .await
            .expect("connect to seeded sqlite");
        let hash = transcoderr::api::auth::hash_password(&raw).expect("hash token");
        let prefix = &raw[..12];
        sqlx::query("INSERT INTO api_tokens (name, hash, prefix, created_at) VALUES (?, ?, ?, ?)")
            .bind("e2e")
            .bind(hash)
            .bind(prefix)
            .bind(chrono::Utc::now().timestamp())
            .execute(&pool)
            .await
            .expect("insert api_token");
        // Enable auth so require_auth runs.
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('auth.enabled','true')")
            .execute(&pool)
            .await
            .expect("set auth.enabled");
        let pw_hash =
            transcoderr::api::auth::hash_password("unused").expect("hash placeholder password");
        sqlx::query(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('auth.password_hash', ?)",
        )
        .bind(pw_hash)
        .execute(&pool)
        .await
        .expect("set auth.password_hash");
    });
}

fn wait_or_kill(child: &mut std::process::Child, ms: Duration) -> std::io::Result<()> {
    let deadline = Instant::now() + ms;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill()
}
