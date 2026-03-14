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
            .env("TMUX_TMPDIR", self.tmpdir.path())
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
            .env("TMUX_TMPDIR", self.tmpdir.path())
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

    // CREATED column (chars 44..64) should have a humanized timestamp (e.g. "now")
    let data_line = output.lines().find(|l| l.contains("tmux://mysession/")).unwrap();
    let created_col = &data_line[44..64];
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
