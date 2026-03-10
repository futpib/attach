use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use tokio::process::Command;

use super::Backend;
use crate::Target;

pub struct TmuxBackend;

fn is_tmux_id(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some('$' | '@' | '%') => chars.all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

fn resolve_target(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    if is_tmux_id(path) {
        return Ok(path.to_string());
    }

    let parts: Vec<&str> = path.splitn(3, '/').collect();
    match parts.len() {
        3 => Ok(format!("{}:{}.{}", parts[0], parts[1], parts[2])),
        _ => Err(format!("invalid tmux target: {}, expected session/window/pane or a tmux ID (e.g. %0, @0, $0)", path).into()),
    }
}

impl Backend for TmuxBackend {
    fn scheme(&self) -> &'static str {
        "tmux"
    }

    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Vec<Target>> + Send + '_>> {
        Box::pin(async {
            let output = Command::new("tmux")
                .args([
                    "list-panes", "-a", "-F",
                    "#{pane_id}\t#{session_name}/#{window_index}/#{pane_index}",
                ])
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
                .filter_map(|line| {
                    let (pane_id, friendly) = line.split_once('\t')?;
                    Some(Target {
                        url: format!("tmux://{}", friendly),
                        aliases: vec![format!("tmux://{}", pane_id)],
                    })
                })
                .collect()
        })
    }

    fn build_command(&self, path: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
        let target = resolve_target(path)?;
        Ok(("tmux".to_string(), vec!["attach-session".to_string(), "-t".to_string(), target]))
    }
}
