use std::path::PathBuf;
use std::process::{Command, Stdio};

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct DockerContainer {
    name: String,
}

impl DockerContainer {
    /// Start a container with `-it` (interactive + TTY).
    fn new(name: &str) -> Self {
        Self::with_flags(name, &["-d", "-it"])
    }

    /// Start a container with `-i` only (interactive, no TTY).
    fn new_no_tty(name: &str) -> Self {
        Self::with_flags(name, &["-d", "-i"])
    }

    fn with_flags(name: &str, flags: &[&str]) -> Self {
        let name = format!("attach-test-{}", name);
        let _ = Command::new("docker")
            .args(["rm", "-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.extend_from_slice(&["--name", &name, "alpine:latest", "sh"]);

        let output = Command::new("docker")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to start docker container");
        assert!(
            output.status.success(),
            "failed to start container {}: {}",
            name,
            String::from_utf8_lossy(&output.stderr),
        );

        std::thread::sleep(std::time::Duration::from_millis(500));

        Self { name }
    }

    fn attach_bin() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_attach"))
    }

    fn attach_cmd(&self, args: &[&str]) -> std::process::Output {
        Command::new(Self::attach_bin())
            .args(args)
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

    fn docker_exec(&self, cmd: &str) -> String {
        let output = Command::new("docker")
            .args(["exec", &self.name, "sh", "-c", cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("failed to docker exec");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn target(&self) -> String {
        format!("docker://{}", self.name)
    }

    fn is_running(&self) -> bool {
        let output = Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", &self.name])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .expect("failed to inspect container");
        String::from_utf8_lossy(&output.stdout).trim() == "true"
    }

    fn process_running(&self, name: &str) -> bool {
        let output = self.docker_exec(&format!("ps | grep '{}' | grep -v grep", name));
        !output.trim().is_empty()
    }
}

impl Drop for DockerContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ==========================================================================
// Tests for -it (TTY) containers — uses PTY path with full signal support
// ==========================================================================

#[test]
fn docker_tty_type_and_key_return() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-type-return");

    container.attach_cmd_ok(&["type", &container.target(), "echo hello_docker > /tmp/out"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    assert_eq!(container.docker_exec("cat /tmp/out"), "hello_docker");
}

#[test]
fn docker_tty_ctrl_c_interrupts_sleep() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-ctrlc");

    container.attach_cmd_ok(&["type", &container.target(), "sleep 999"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_secs(1));

    assert!(container.process_running("sleep"), "sleep should be running");

    container.attach_cmd_ok(&["key", &container.target(), "ctrl+c"]);
    std::thread::sleep(std::time::Duration::from_secs(1));

    assert!(!container.process_running("sleep"), "sleep should be killed by ctrl+c");
    assert!(container.is_running(), "container should survive ctrl+c");
}

#[test]
fn docker_tty_commands_after_ctrl_c() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-after-ctrlc");

    // Start and interrupt a process
    container.attach_cmd_ok(&["type", &container.target(), "sleep 999"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_secs(1));
    container.attach_cmd_ok(&["key", &container.target(), "ctrl+c"]);
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Should still accept commands
    container.attach_cmd_ok(&["type", &container.target(), "echo recovered > /tmp/out"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_millis(500));

    assert_eq!(container.docker_exec("cat /tmp/out"), "recovered");
}

#[test]
fn docker_tty_multiple_commands() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-multi");

    container.attach_cmd_ok(&["type", &container.target(), "echo first > /tmp/a"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    container.attach_cmd_ok(&["type", &container.target(), "echo second > /tmp/b"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    assert_eq!(container.docker_exec("cat /tmp/a"), "first");
    assert_eq!(container.docker_exec("cat /tmp/b"), "second");
}

#[test]
fn docker_tty_special_characters() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-special");

    container.attach_cmd_ok(&["type", &container.target(), "echo 'hello;world' > /tmp/special"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    assert_eq!(container.docker_exec("cat /tmp/special"), "hello;world");
}

#[test]
fn docker_tty_shell_pipeline() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-pipe");

    container.attach_cmd_ok(&["type", &container.target(), "echo hello | tr a-z A-Z > /tmp/pipe"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    assert_eq!(container.docker_exec("cat /tmp/pipe"), "HELLO");
}

#[test]
fn docker_tty_survives_many_iterations() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("tty-survive");

    for i in 0..5 {
        container.attach_cmd_ok(&[
            "type",
            &container.target(),
            &format!("echo iter_{} > /tmp/iter", i),
        ]);
        container.attach_cmd_ok(&["key", &container.target(), "Return"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    assert!(container.is_running());
    assert_eq!(container.docker_exec("cat /tmp/iter"), "iter_4");
}

// ==========================================================================
// Tests for -i (no TTY) containers — uses pipe path
// ==========================================================================

#[test]
fn docker_notty_type_and_key_return() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new_no_tty("notty-type-return");

    container.attach_cmd_ok(&["type", &container.target(), "echo notty_works > /tmp/out"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    assert_eq!(container.docker_exec("cat /tmp/out"), "notty_works");
}

#[test]
fn docker_notty_multiple_commands() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new_no_tty("notty-multi");

    container.attach_cmd_ok(&["type", &container.target(), "echo first > /tmp/a"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    container.attach_cmd_ok(&["type", &container.target(), "echo second > /tmp/b"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);
    std::thread::sleep(std::time::Duration::from_millis(300));

    assert_eq!(container.docker_exec("cat /tmp/a"), "first");
    assert_eq!(container.docker_exec("cat /tmp/b"), "second");
}

#[test]
fn docker_notty_preserves_spaces() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new_no_tty("notty-spaces");

    container.attach_cmd_ok(&["type", &container.target(), "echo 'hello   world' > /tmp/spaces"]);
    container.attach_cmd_ok(&["key", &container.target(), "Return"]);

    std::thread::sleep(std::time::Duration::from_millis(500));

    assert_eq!(container.docker_exec("cat /tmp/spaces"), "hello   world");
}

// ==========================================================================
// Common tests (scheme-level)
// ==========================================================================

#[test]
fn docker_ps_shows_container() {
    if !docker_available() {
        return;
    }

    let container = DockerContainer::new("ps-test");

    let output = container.attach_cmd_ok(&["ps", "-q"]);

    assert!(
        output.contains(&format!("docker://{}", container.name)),
        "expected docker://{} in ps output, got:\n{}",
        container.name,
        output,
    );
}

#[test]
fn docker_invalid_container_key_fails() {
    if !docker_available() {
        return;
    }

    let output = Command::new(DockerContainer::attach_bin())
        .args(["key", "docker://nonexistent-container-xyz", "Return"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run attach");

    assert!(!output.status.success());
}

#[test]
fn docker_invalid_container_type_fails() {
    if !docker_available() {
        return;
    }

    let output = Command::new(DockerContainer::attach_bin())
        .args(["type", "docker://nonexistent-container-xyz", "hello"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run attach");

    assert!(!output.status.success());
}
