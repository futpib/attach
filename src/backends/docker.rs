use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use tokio::process::Command;

use super::Backend;
use crate::Target;

pub struct DockerBackend;

#[derive(serde::Deserialize)]
struct DockerContainer {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Labels")]
    labels: String,
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

    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Vec<Target>> + Send + '_>> {
        Box::pin(async {
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
        })
    }

    fn build_command(&self, path: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
        let container = resolve_container(path)?;
        Ok(("docker".to_string(), vec!["attach".to_string(), container]))
    }
}
