use std::process::Stdio;

use clap::{Parser, Subcommand};
use tokio::process::Command;

#[derive(Parser)]
#[command(name = "attach", about = "Manage attachable terminals")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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

fn attach_docker(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    let container = resolve_docker_container(path)?;
    let err = std::process::Command::new("docker")
        .args(["attach", &container])
        .exec();
    Err(err.into())
}

fn attach_tmux(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    let parts: Vec<&str> = path.splitn(3, '/').collect();
    let (session, window, pane) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        _ => return Err(format!("invalid tmux target: {}, expected session/window/pane", path).into()),
    };

    let target = format!("{}:{}.{}", session, window, pane);
    let err = std::process::Command::new("tmux")
        .args(["attach-session", "-t", &target])
        .exec();
    Err(err.into())
}

fn attach(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(path) = target.strip_prefix("docker://") {
        attach_docker(path)
    } else if let Some(path) = target.strip_prefix("tmux://") {
        attach_tmux(path)
    } else {
        Err(format!("unknown target scheme: {}", target).into())
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
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
    }
}
