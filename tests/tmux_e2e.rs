use std::path::PathBuf;
use std::process::{Command, Stdio};

struct TmuxServer {
    tmpdir: tempfile::TempDir,
}

impl TmuxServer {
    fn new() -> Self {
        let tmpdir = tempfile::tempdir().expect("failed to create tmpdir");
        Self { tmpdir }
    }

    fn tmux(&self, args: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .env_clear()
            .env("TMUX_TMPDIR", self.tmpdir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .arg("-f")
            .arg("/dev/null")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to run tmux")
    }

    fn tmux_ok(&self, args: &[&str]) -> String {
        let output = self.tmux(args);
        assert!(
            output.status.success(),
            "tmux {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn attach_bin() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_attach"))
    }

    fn attach_cmd(&self, args: &[&str]) -> std::process::Output {
        Command::new(Self::attach_bin())
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("TMUX_TMPDIR", self.tmpdir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to run attach")
    }

    fn attach_cmd_ok(&self, args: &[&str]) -> String {
        let output = self.attach_cmd(args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "attach {:?} failed (exit {:?}):\nstdout: {}\nstderr: {}",
            args,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            stderr,
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .env_clear()
            .env("TMUX_TMPDIR", self.tmpdir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .args(["kill-server"])
            .output();
    }
}

#[test]
fn ps_lists_tmux_panes() {
    let server = TmuxServer::new();

    server.tmux_ok(&["new-session", "-d", "-s", "test-session", "sleep", "300"]);

    let output = server.attach_cmd_ok(&["ps", "-q"]);

    assert!(
        output.contains("tmux://test-session/"),
        "expected tmux://test-session/ in output, got:\n{}",
        output,
    );
}

#[test]
fn ps_table_shows_tmux_panes() {
    let server = TmuxServer::new();

    server.tmux_ok(&["new-session", "-d", "-s", "mysession", "sleep", "300"]);

    let output = server.attach_cmd_ok(&["ps"]);

    // Should have header
    assert!(output.contains("ID"), "missing ID header in:\n{}", output);
    assert!(output.contains("COMMAND"), "missing COMMAND header in:\n{}", output);
    assert!(output.contains("CREATED"), "missing CREATED header in:\n{}", output);
    assert!(output.contains("NAME"), "missing NAME header in:\n{}", output);

    // Should have a tmux pane row
    assert!(
        output.contains("tmux://mysession/"),
        "expected tmux://mysession/ in output, got:\n{}",
        output,
    );

    // ID column should have tmux://%N format
    assert!(
        output.contains("tmux://%"),
        "expected tmux://%%N id in output, got:\n{}",
        output,
    );

    // CREATED column (chars 62..82) should have a humanized timestamp (e.g. "now")
    let data_line = output.lines().find(|l| l.contains("tmux://mysession/")).unwrap();
    let created_col = &data_line[62..82];
    assert!(
        created_col.trim() != "",
        "expected non-empty CREATED value, got line:\n{}",
        data_line,
    );
}

#[test]
fn ps_multiple_panes() {
    let server = TmuxServer::new();

    server.tmux_ok(&["new-session", "-d", "-s", "multi", "sleep", "300"]);
    server.tmux_ok(&["split-window", "-t", "multi", "sleep", "300"]);

    let output = server.attach_cmd_ok(&["ps", "-q"]);

    let tmux_lines: Vec<&str> = output.lines().filter(|l| l.starts_with("tmux://")).collect();
    assert!(
        tmux_lines.len() >= 2,
        "expected at least 2 tmux panes, got {}:\n{}",
        tmux_lines.len(),
        output,
    );
}

#[test]
fn ps_no_tmux_server_succeeds() {
    let tmpdir = tempfile::tempdir().expect("failed to create tmpdir");
    let output = Command::new(TmuxServer::attach_bin())
        .args(["ps", "-q"])
        .env("TMUX_TMPDIR", tmpdir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run attach");

    assert!(
        output.status.success(),
        "attach ps should succeed without tmux server, stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    // Should not print warnings about tmux not running
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("warning"),
        "should not warn when tmux server is not running, stderr: {}",
        stderr,
    );
}

#[test]
fn type_sends_literal_text() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "typetest", "-x", "80", "-y", "24"]);

    // Wait for shell prompt
    std::thread::sleep(std::time::Duration::from_millis(500));

    server.attach_cmd_ok(&["type", "tmux://typetest/0/0", "echo", "hello_from_type"]);
    server.attach_cmd_ok(&["key", "tmux://typetest/0/0", "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let captured = server.tmux_ok(&["capture-pane", "-t", "typetest", "-p"]);
    assert!(
        captured.contains("hello_from_type"),
        "expected 'hello_from_type' in pane output, got:\n{}",
        captured,
    );
}

#[test]
fn key_sends_return() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "keytest", "-x", "80", "-y", "24"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Type a command, then send Return via key subcommand
    server.tmux_ok(&["send-keys", "-t", "keytest", "-l", "echo key_return_works"]);
    server.attach_cmd_ok(&["key", "tmux://keytest/0/0", "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let captured = server.tmux_ok(&["capture-pane", "-t", "keytest", "-p"]);
    assert!(
        captured.contains("key_return_works"),
        "expected 'key_return_works' in pane output, got:\n{}",
        captured,
    );
}

#[test]
fn key_ctrl_c_interrupts() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "ctrlctest", "-x", "80", "-y", "24"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Start a long sleep
    server.tmux_ok(&["send-keys", "-t", "ctrlctest", "-l", "sleep 999"]);
    server.tmux_ok(&["send-keys", "-t", "ctrlctest", "Enter"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Send ctrl+c via attach key
    server.attach_cmd_ok(&["key", "tmux://ctrlctest/0/0", "ctrl+c"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Pane should show a new prompt (sleep was interrupted)
    let captured = server.tmux_ok(&["capture-pane", "-t", "ctrlctest", "-p"]);
    // After ctrl+c the shell prints ^C and gives a new prompt
    assert!(
        captured.contains("^C") || captured.lines().filter(|l| !l.trim().is_empty()).count() >= 2,
        "expected ctrl+c to interrupt sleep, got:\n{}",
        captured,
    );
}

#[test]
fn key_ctrl_a_and_ctrl_k() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "ctrlaktest", "-x", "80", "-y", "24"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Type some text
    server.attach_cmd_ok(&["type", "tmux://ctrlaktest/0/0", "delete_this"]);

    // ctrl+a moves to beginning of line, ctrl+k kills to end
    server.attach_cmd_ok(&["key", "tmux://ctrlaktest/0/0", "ctrl+a"]);
    server.attach_cmd_ok(&["key", "tmux://ctrlaktest/0/0", "ctrl+k"]);

    // Now type replacement text and execute
    server.attach_cmd_ok(&["type", "tmux://ctrlaktest/0/0", "echo replaced_text"]);
    server.attach_cmd_ok(&["key", "tmux://ctrlaktest/0/0", "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let captured = server.tmux_ok(&["capture-pane", "-t", "ctrlaktest", "-p"]);
    assert!(
        captured.contains("replaced_text"),
        "expected 'replaced_text' in pane output, got:\n{}",
        captured,
    );
    // The original text should not appear as executed output
    assert!(
        !captured.contains("delete_this"),
        "original text 'delete_this' should have been cleared, got:\n{}",
        captured,
    );
}

#[test]
fn key_multiple_keys_in_one_call() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "multikey", "-x", "80", "-y", "24"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Type text then send ctrl+a ctrl+k in separate calls, then type+enter in one flow
    server.attach_cmd_ok(&["type", "tmux://multikey/0/0", "echo multi_key_test"]);
    server.attach_cmd_ok(&["key", "tmux://multikey/0/0", "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let captured = server.tmux_ok(&["capture-pane", "-t", "multikey", "-p"]);
    assert!(
        captured.contains("multi_key_test"),
        "expected 'multi_key_test' in pane output, got:\n{}",
        captured,
    );
}

#[test]
fn key_with_pane_id_target() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "paneidtest", "-x", "80", "-y", "24"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Get the pane ID
    let pane_id = server.tmux_ok(&["display-message", "-t", "paneidtest", "-p", "#{pane_id}"]);

    // Use pane ID as target
    let target = format!("tmux://{}", pane_id);
    server.attach_cmd_ok(&["type", &target, "echo pane_id_works"]);
    server.attach_cmd_ok(&["key", &target, "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let captured = server.tmux_ok(&["capture-pane", "-t", "paneidtest", "-p"]);
    assert!(
        captured.contains("pane_id_works"),
        "expected 'pane_id_works' in pane output, got:\n{}",
        captured,
    );
}

#[test]
fn type_special_characters() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "specialtest", "-x", "80", "-y", "24"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Type text with special characters (quotes, semicolons)
    server.attach_cmd_ok(&["type", "tmux://specialtest/0/0", "echo 'hello;world'"]);
    server.attach_cmd_ok(&["key", "tmux://specialtest/0/0", "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    let captured = server.tmux_ok(&["capture-pane", "-t", "specialtest", "-p"]);
    assert!(
        captured.contains("hello;world"),
        "expected 'hello;world' in pane output, got:\n{}",
        captured,
    );
}

#[test]
fn key_invalid_target_fails() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "dummy", "sleep", "300"]);

    let output = server.attach_cmd(&["key", "tmux://nonexistent/0/0", "Return"]);
    assert!(
        !output.status.success(),
        "expected failure for invalid target",
    );
}

#[test]
fn type_invalid_target_fails() {
    let server = TmuxServer::new();
    server.tmux_ok(&["new-session", "-d", "-s", "dummy", "sleep", "300"]);

    let output = server.attach_cmd(&["type", "tmux://nonexistent/0/0", "hello"]);
    assert!(
        !output.status.success(),
        "expected failure for invalid target",
    );
}

#[test]
fn key_invalid_scheme_fails() {
    let server = TmuxServer::new();

    let output = server.attach_cmd(&["key", "bogus://foo", "Return"]);
    assert!(
        !output.status.success(),
        "expected failure for unknown scheme",
    );
}
