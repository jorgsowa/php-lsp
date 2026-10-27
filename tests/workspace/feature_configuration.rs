//! workspace/didChangeConfiguration: PHP version detection, validation,
//! semantic-token refresh, and repeated calls.

use super::*;

use expect_test::expect;
use serde_json::json;

fn extract_log_message(notif: &serde_json::Value) -> String {
    notif["params"]["message"].as_str().unwrap_or("").to_owned()
}

#[tokio::test]
async fn change_configuration_valid_php_version_is_logged() {
    let mut server = TestServer::new().await;
    let log = server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;
    let msg = extract_log_message(&log);
    expect!["php-lsp: using PHP 8.3 (set by editor)"].assert_eq(&msg);
}

#[tokio::test]
async fn change_configuration_invalid_php_version_logs_warning() {
    let mut server = TestServer::new().await;

    server
        .client()
        .notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": null }),
        )
        .await;
    let (req_id, _) = server
        .client()
        .expect_server_request("workspace/configuration")
        .await;
    server
        .client()
        .reply_to_server_request(req_id, json!([{ "phpVersion": "5.6" }]))
        .await;

    let warning_msg = server.client().read_notification("window/logMessage").await;
    let warning_text = extract_log_message(&warning_msg);
    expect![[
        r#"php-lsp: unsupported phpVersion "5.6" — valid values: 7.4, 8.0, 8.1, 8.2, 8.3, 8.4, 8.5, 8.6"#
    ]]
    .assert_eq(&warning_text);

    // Invalid versions skip environment detection and fall straight to the
    // latest stub (PHP_8_5), which resolve_php_version reports as explicit
    // ("set by editor") since from_value already set cfg.php_version — this
    // is deterministic, not environment-dependent.
    let info_msg = server.client().read_notification("window/logMessage").await;
    let info_text = extract_log_message(&info_msg);
    expect!["php-lsp: using PHP 8.5 (set by editor)"].assert_eq(&info_text);
}

#[tokio::test]
async fn change_configuration_triggers_semantic_token_refresh() {
    let mut server = TestServer::new().await;

    server
        .client()
        .notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": null }),
        )
        .await;
    let (req_id, _) = server
        .client()
        .expect_server_request("workspace/configuration")
        .await;
    server
        .client()
        .reply_to_server_request(req_id, json!([{ "phpVersion": "8.1" }]))
        .await;

    let _log = server.client().read_notification("window/logMessage").await;

    let (refresh_id, _) = server
        .client()
        .expect_server_request("workspace/semanticTokens/refresh")
        .await;
    server
        .client()
        .reply_to_server_request(refresh_id, json!(null))
        .await;
}

#[tokio::test]
async fn change_configuration_can_be_called_twice() {
    let mut server = TestServer::new().await;

    let log1 = server
        .change_configuration(json!({ "phpVersion": "8.1" }))
        .await;
    let msg1 = extract_log_message(&log1);
    expect!["php-lsp: using PHP 8.1 (set by editor)"].assert_eq(&msg1);

    let log2 = server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;
    let msg2 = extract_log_message(&log2);
    expect!["php-lsp: using PHP 8.3 (set by editor)"].assert_eq(&msg2);
}

#[tokio::test]
async fn change_configuration_empty_config_uses_detected_version() {
    let mut server = TestServer::new().await;

    let log = server.change_configuration(json!({})).await;
    let msg = extract_log_message(&log);
    assert!(
        msg.starts_with("php-lsp: using PHP "),
        "expected version log: {msg:?}"
    );
    assert!(
        !msg.contains("set by editor"),
        "empty config must not claim 'set by editor': {msg:?}"
    );
}

/// A runtime PHP-version change must re-populate the workspace file scope,
/// or `workspace_file_paths()` — and everything scoped by it: references,
/// rename, workspace symbols — silently sees an empty workspace afterward.
/// Regression test: `DocumentStore::set_php_version`'s
/// `drop_session_scoped_state` clears `lsp_ws_files` on every real version
/// change, and nothing repopulated it before this fix.
#[tokio::test]
async fn change_configuration_php_version_change_rescans_workspace() {
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({ "diagnostics": {"enabled": false}, "phpVersion": "8.1" }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Sanity: workspace symbols resolve before the version change.
    server
        .wait_until_symbol_present("User", std::time::Duration::from_secs(5))
        .await;

    server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;

    // Without the fix this call is answered from an emptied workspace and
    // times out; the re-scan the fix adds repopulates it.
    server
        .wait_until_symbol_present("User", std::time::Duration::from_secs(5))
        .await;

    let out = server.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       User @ src/Model/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn indexed_extensions_index_drupal_files_and_resolve_includes() {
    let module = "<?php\ninclude_once __DIR__ . '/helpers.inc';\nfunction mymod_boot() { drupal_helper(); }\n";
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mymod.module"), module).unwrap();
    std::fs::write(
        tmp.path().join("helpers.inc"),
        "<?php\nfunction drupal_helper(): void {}\n",
    )
    .unwrap();

    let mut s = TestServer::with_root_and_options(
        tmp.path(),
        json!({ "indexedExtensions": ["php", "module", "inc"] }),
    )
    .await;
    s.wait_for_index_ready().await;

    let symbols = s.snapshot_workspace_symbols("drupal_helper").await;
    assert!(symbols.contains("helpers.inc"), "symbols: {symbols}");

    s.open("mymod.module", module).await;
    let diags = s.pull_diagnostics("mymod.module").await;
    assert!(
        !diags.to_string().contains("UndefinedFunction"),
        "diagnostics: {diags}"
    );
}

#[tokio::test]
async fn default_extensions_skip_drupal_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("helpers.inc"),
        "<?php\nfunction drupal_helper(): void {}\n",
    )
    .unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    let symbols = s.snapshot_workspace_symbols("drupal_helper").await;
    assert!(!symbols.contains("helpers.inc"), "symbols: {symbols}");
}

/// Writes an executable script that creates `marker` when run, standing in
/// for a `phpstan` binary.
#[cfg(unix)]
fn marker_script(dir: &std::path::Path, marker: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("fake-phpstan.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
async fn save_and_check_marker(s: &mut TestServer, marker: &std::path::Path) -> bool {
    s.open("a.php", "<?php\necho 1;\n").await;
    s.save("a.php").await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if marker.exists() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    false
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(external_tool_slots)]
async fn project_config_cannot_run_external_tool_binaries() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let marker = tools.path().join("ran");
    let script = marker_script(tools.path(), &marker);
    std::fs::write(tmp.path().join("a.php"), "<?php\necho 1;\n").unwrap();
    std::fs::write(
        tmp.path().join(".php-lsp.json"),
        json!({"externalTools": {"phpstan": {"enabled": true, "binPath": script}}}).to_string(),
    )
    .unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    assert!(
        !save_and_check_marker(&mut s, &marker).await,
        "a binPath from .php-lsp.json must not be executed"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(external_tool_slots)]
async fn editor_config_runs_external_tool_binaries() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let marker = tools.path().join("ran");
    let script = marker_script(tools.path(), &marker);
    std::fs::write(tmp.path().join("a.php"), "<?php\necho 1;\n").unwrap();

    let mut s = TestServer::with_root_and_options(
        tmp.path(),
        json!({"externalTools": {"phpstan": {"enabled": true, "binPath": script}}}),
    )
    .await;
    s.wait_for_index_ready().await;
    assert!(
        save_and_check_marker(&mut s, &marker).await,
        "a binPath from editor settings must run on save"
    );
}

#[cfg(unix)]
fn tool_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("fake-tool.sh");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
fn process_is_running(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[cfg(unix)]
async fn poll_until(timeout: std::time::Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    cond()
}

/// A fake tool that records its own pid and that of a forked `sleep` child,
/// one `<shell> <child>` line per run, then waits on the child.
#[cfg(unix)]
fn forking_tool_script(dir: &std::path::Path, pids: &std::path::Path) -> std::path::PathBuf {
    tool_script(
        dir,
        &format!("sleep 30 &\necho \"$$ $!\" >> '{}'\nwait", pids.display()),
    )
}

#[cfg(unix)]
fn read_tool_pids(pids: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(pids)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(external_tool_slots)]
async fn external_tool_is_killed_after_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let pids = tools.path().join("pids");
    let script = forking_tool_script(tools.path(), &pids);
    std::fs::write(tmp.path().join("a.php"), "<?php\necho 1;\n").unwrap();

    let mut s = TestServer::with_root_and_options(
        tmp.path(),
        json!({"externalTools": {
            "timeoutMs": 3000,
            "phpstan": {"enabled": true, "binPath": script},
        }}),
    )
    .await;
    s.wait_for_index_ready().await;
    s.open("a.php", "<?php\necho 1;\n").await;
    s.save("a.php").await;

    let secs = std::time::Duration::from_secs(10);
    assert!(
        poll_until(secs, || read_tool_pids(&pids).len() == 2).await,
        "tool never started"
    );
    for pid in read_tool_pids(&pids) {
        assert!(
            poll_until(secs, || !process_is_running(&pid)).await,
            "tool process {pid} still running after its timeout"
        );
    }
}

/// A newer save or an edit supersedes the running tool: its processes die
/// long before the timeout and its slot frees up.
#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(external_tool_slots)]
async fn superseded_external_tool_run_is_killed() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let pids = tools.path().join("pids");
    let script = forking_tool_script(tools.path(), &pids);
    std::fs::write(tmp.path().join("a.php"), "<?php\necho 1;\n").unwrap();

    let mut s = TestServer::with_root_and_options(
        tmp.path(),
        json!({"externalTools": {
            "timeoutMs": 120000,
            "phpstan": {"enabled": true, "binPath": script},
        }}),
    )
    .await;
    s.wait_for_index_ready().await;
    s.open("a.php", "<?php\necho 1;\n").await;

    let secs = std::time::Duration::from_secs(10);
    s.save("a.php").await;
    assert!(
        poll_until(secs, || read_tool_pids(&pids).len() == 2).await,
        "first run never started"
    );
    s.save("a.php").await;
    assert!(
        poll_until(secs, || read_tool_pids(&pids).len() == 4).await,
        "second run never started"
    );
    let all = read_tool_pids(&pids);
    for pid in &all[..2] {
        assert!(
            poll_until(secs, || !process_is_running(pid)).await,
            "process {pid} of the superseded run is still running"
        );
    }

    s.change("a.php", 2, "<?php\necho 2;\n").await;
    for pid in &all[2..] {
        assert!(
            poll_until(secs, || !process_is_running(pid)).await,
            "process {pid} of the run for the edited buffer is still running"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(external_tool_slots)]
async fn external_tool_runs_are_capped() {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let running = tools.path().join("running");
    std::fs::create_dir(&running).unwrap();
    let log = tools.path().join("log");
    let script = tool_script(
        tools.path(),
        &format!(
            "touch '{run}/'$$\nls '{run}' | wc -l >> '{log}'\nsleep 0.4\nrm '{run}/'$$",
            run = running.display(),
            log = log.display()
        ),
    );
    const SAVES: usize = 6;
    for i in 0..SAVES {
        std::fs::write(tmp.path().join(format!("a{i}.php")), "<?php\necho 1;\n").unwrap();
    }

    let mut s = TestServer::with_root_and_options(
        tmp.path(),
        json!({"externalTools": {"phpstan": {"enabled": true, "binPath": script}}}),
    )
    .await;
    s.wait_for_index_ready().await;
    for i in 0..SAVES {
        let path = format!("a{i}.php");
        s.open(&path, "<?php\necho 1;\n").await;
        s.save(&path).await;
    }

    let read_counts = || -> Vec<usize> {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    };
    assert!(
        poll_until(std::time::Duration::from_secs(10), || read_counts().len()
            == SAVES)
        .await,
        "expected {SAVES} runs, got {:?}",
        read_counts()
    );
    let max = read_counts().into_iter().max().unwrap();
    assert!(max <= 2, "{max} tool runs overlapped");
}
