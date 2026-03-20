use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use tokio::process::Command;

use super::Backend;
use crate::Target;

pub struct DockerBackend;

struct DockerContainer {
    id: String,
    names: String,
    labels: String,
    command: String,
    created_at: String,
}

fn resolve_container(path: &str) -> Result<String, String> {
    let parts: Vec<&str> = path.splitn(2, '/').collect();
    match parts.len() {
        1 => Ok(parts[0].to_string()),
        2 => {
            let project = parts[0];
            let service = parts[1];
            Ok(format!("{}-{}", project, service))
        }
        _ => Err(format!("invalid docker target: {}", path)),
    }
}

fn container_has_tty(container: &str) -> bool {
    let output = std::process::Command::new("docker")
        .args(["inspect", "-f", "{{.Config.Tty}}", container])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "true",
        Err(_) => false,
    }
}

/// Write data through a raw PTY via `docker attach`. The PTY is set to raw
/// mode so control bytes (\x03 for ctrl+c, etc.) pass through to the container's
/// TTY where they generate the proper signals.
/// Only works for containers started with `-t` (TTY mode).
fn docker_write_pty(container: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // Open a PTY pair
    let mut master_fd: libc::c_int = 0;
    let mut slave_fd: libc::c_int = 0;
    let ret = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ret != 0 {
        return Err("openpty failed".into());
    }
    let master = unsafe { OwnedFd::from_raw_fd(master_fd) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };

    // Set the slave to raw mode so control chars pass through as data
    // to docker attach, which forwards them to the container's TTY
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        libc::tcgetattr(slave.as_raw_fd(), &mut termios);
        libc::cfmakeraw(&mut termios);
        libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios);
    }

    // Spawn docker attach with the slave as stdin/stdout/stderr
    let child = unsafe {
        std::process::Command::new("docker")
            .args(["attach", "--detach-keys=ctrl-]", container])
            .stdin(Stdio::from(OwnedFd::from_raw_fd(libc::dup(slave.as_raw_fd()))))
            .stdout(Stdio::from(OwnedFd::from_raw_fd(libc::dup(slave.as_raw_fd()))))
            .stderr(Stdio::from(OwnedFd::from_raw_fd(libc::dup(slave.as_raw_fd()))))
            .spawn()
    };
    drop(slave);
    let mut child = child?;

    // Read master output in background thread
    let master_read_fd = unsafe { OwnedFd::from_raw_fd(libc::dup(master.as_raw_fd())) };
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(master_read_fd);
        let mut buf = [0u8; 4096];
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Write data to the master side — in raw mode, bytes pass through verbatim
    let written = unsafe {
        libc::write(
            master.as_raw_fd(),
            data.as_ptr() as *const libc::c_void,
            data.len(),
        )
    };
    if written < 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("write to PTY master failed".into());
    }

    // Wait for echo: once we see our data echoed back (or timeout), the
    // container has received the input
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut total_received = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(std::time::Duration::from_millis(100))) {
            Ok(chunk) => {
                total_received += chunk.len();
                if total_received >= data.len() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if total_received > 0 {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Send detach sequence (ctrl+] = 0x1d) to cleanly disconnect
    unsafe {
        let detach: [u8; 1] = [0x1d];
        libc::write(master.as_raw_fd(), detach.as_ptr() as *const libc::c_void, 1);
    }

    drop(master);

    let child_thread = std::thread::spawn(move || {
        match child.try_wait() {
            Ok(Some(_)) => return,
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = child.kill();
        let _ = child.wait();
    });
    let _ = child_thread.join();
    Ok(())
}

/// Write data directly to the container's main process stdin via /proc/1/fd/0.
/// Fast and reliable for text + newlines, but control characters (ctrl+c, etc.)
/// are delivered as literal bytes without TTY signal processing.
fn docker_write_pipe(container: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;

    let mut child = std::process::Command::new("docker")
        .args(["exec", "-i", container, "sh", "-c", "cat > /proc/1/fd/0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Translate \r to \n since there's no TTY layer
        let translated: Vec<u8> = data.iter().map(|&b| if b == b'\r' { b'\n' } else { b }).collect();
        stdin.write_all(&translated)?;
        stdin.flush()?;
        drop(stdin);
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker exec failed: {}", stderr.trim()).into());
    }
    Ok(())
}

fn docker_write_stdin(container: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if container_has_tty(container) {
        docker_write_pty(container, data)
    } else {
        docker_write_pipe(container, data)
    }
}

impl Backend for DockerBackend {
    fn scheme(&self) -> &'static str {
        "docker"
    }

    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Target>, Box<dyn std::error::Error + Send + Sync>>> + Send + '_>> {
        Box::pin(async {
        let format_str = [
            "{{.ID}}",
            "{{.Names}}",
            "{{.Labels}}",
            "{{.Command}}",
            "{{.CreatedAt}}",
        ].join(";;");
        let output = Command::new("docker")
            .args(["ps", "--format", &format_str])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("docker ps failed: {}", stderr.trim()).into());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut targets = Vec::new();

        for line in stdout.lines() {
            let mut parts = line.splitn(5, ";;");
            let container = match (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some(id), Some(names), Some(labels), Some(command), Some(created_at)) => {
                    DockerContainer {
                        id: id.to_string(),
                        names: names.to_string(),
                        labels: labels.to_string(),
                        command: command.to_string(),
                        created_at: created_at.to_string(),
                    }
                }
                _ => continue,
            };

            let labels: std::collections::HashMap<&str, &str> = container
                .labels
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .collect();

            let project = labels.get("com.docker.compose.project").copied();
            let service = labels.get("com.docker.compose.service").copied();

            let url = match (project, service) {
                (Some(p), Some(s)) => format!("docker://{}/{}", p, s),
                _ => format!("docker://{}", container.names.split(',').next().unwrap_or(&container.id)),
            };

            let mut aliases = vec![format!("docker://{}", container.id)];
            let name = container.names.split(',').next().unwrap_or(&container.id);
            let name_alias = format!("docker://{}", name);
            if name_alias != url {
                aliases.push(name_alias);
            }

            let created = chrono::DateTime::parse_from_str(
                    &container.created_at,
                    "%Y-%m-%d %H:%M:%S %z %Z",
                )
                .or_else(|_| chrono::DateTime::parse_from_str(
                    &container.created_at,
                    "%Y-%m-%d %H:%M:%S %z",
                ))
                .map(|dt| chrono_humanize::HumanTime::from(dt).to_string())
                .unwrap_or(container.created_at.clone());

            targets.push(Target {
                id: format!("docker://{}", container.id),
                command: container.command.clone(),
                created,
                url,
                aliases,
            });
        }

        Ok(targets)
        })
    }

    fn build_command(&self, path: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
        let container = resolve_container(path)?;
        Ok(("docker".to_string(), vec!["attach".to_string(), container]))
    }

    fn send_keys(&self, path: &str, keys: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        let container = resolve_container(path).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        let mut bytes = Vec::new();
        for key in keys {
            bytes.extend_from_slice(&super::keys::key_to_bytes(key));
        }
        docker_write_stdin(&container, &bytes)
    }

    fn send_text(&self, path: &str, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        let container = resolve_container(path).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        docker_write_stdin(&container, text.as_bytes())
    }
}
