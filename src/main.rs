use std::io::Read as _;
use std::process::Stdio;
use std::time::Duration;

use clap::{Parser, Subcommand};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::process::Command;

#[derive(Parser)]
#[command(name = "attach", about = "Manage attachable terminals")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Target URL (used when no subcommand is given)
    target: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// List all attachable targets
    Ls,
    /// Attach to a target
    Attach {
        /// Target URL (e.g. docker://name, docker://project/service, tmux://session/window/pane)
        target: String,
    },
    /// Print one frame of the target's terminal output
    Screenshot {
        /// Target URL (e.g. docker://name, docker://project/service, tmux://session/window/pane)
        target: String,
    },
}

fn is_target_url(s: &str) -> bool {
    s.starts_with("docker://") || s.starts_with("tmux://")
}

#[derive(serde::Deserialize)]
struct DockerContainer {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Labels")]
    labels: String,
}

struct Target {
    url: String,
}

async fn list_docker_targets() -> Vec<Target> {
    let output = Command::new("docker")
        .args(["ps", "--format", "{{json .}}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut targets = Vec::new();

    for line in stdout.lines() {
        let container: DockerContainer = match serde_json::from_str(line) {
            Ok(c) => c,
            Err(_) => continue,
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

        targets.push(Target { url });
    }

    targets
}

async fn list_tmux_targets() -> Vec<Target> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{session_name}/#{window_index}/#{pane_index}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| Target {
            url: format!("tmux://{}", line),
        })
        .collect()
}

fn resolve_docker_container(path: &str) -> Result<String, String> {
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

fn build_target_command(target: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
    if let Some(path) = target.strip_prefix("docker://") {
        let container = resolve_docker_container(path)?;
        Ok(("docker".to_string(), vec!["attach".to_string(), container]))
    } else if let Some(path) = target.strip_prefix("tmux://") {
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        let (session, window, pane) = match parts.len() {
            3 => (parts[0], parts[1], parts[2]),
            _ => return Err(format!("invalid tmux target: {}, expected session/window/pane", path).into()),
        };
        let tmux_target = format!("{}:{}.{}", session, window, pane);
        Ok(("tmux".to_string(), vec!["attach-session".to_string(), "-t".to_string(), tmux_target]))
    } else {
        Err(format!("unknown target scheme: {}", target).into())
    }
}

fn attach(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    let (program, args) = build_target_command(target)?;
    let err = std::process::Command::new(&program)
        .args(&args)
        .exec();
    Err(err.into())
}

fn screenshot(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (program, args) = build_target_command(target)?;

    let (cols, rows) = terminal_size::terminal_size()
        .map(|(w, h)| (w.0, h.0))
        .unwrap_or((80, 24));

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(&program);
    for arg in &args {
        cmd.arg(arg);
    }

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
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

    let mut parser = vt100::Parser::new(rows, cols, 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(data) => parser.process(&data),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    let screen = parser.screen().contents_formatted();
    let mut stdout = std::io::stdout();
    std::io::Write::write_all(&mut stdout, &screen)?;
    // Reset terminal state that the captured program may have changed
    std::io::Write::write_all(&mut stdout, b"\x1b[?25h\x1b[m")?;
    println!();

    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let command = match (cli.command, cli.target) {
        (Some(cmd), _) => cmd,
        (None, Some(target)) if is_target_url(&target) => Commands::Attach { target },
        (None, _) => Commands::Ls,
    };

    match command {
        Commands::Ls => {
            let (docker_targets, tmux_targets) =
                tokio::join!(list_docker_targets(), list_tmux_targets());

            for target in docker_targets.iter().chain(tmux_targets.iter()) {
                println!("{}", target.url);
            }
        }
        Commands::Attach { target } => {
            if let Err(e) = attach(&target) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Screenshot { target } => {
            if let Err(e) = screenshot(&target) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
