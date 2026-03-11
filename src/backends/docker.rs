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
}
