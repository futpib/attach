use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use tokio::process::Command;

use super::Backend;
use crate::Target;

pub struct TmuxBackend;

impl Backend for TmuxBackend {
    fn scheme(&self) -> &'static str {
        "tmux"
    }

    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Vec<Target>> + Send + '_>> {
        Box::pin(async {
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
        })
    }

    fn build_command(&self, path: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        let (session, window, pane) = match parts.len() {
            3 => (parts[0], parts[1], parts[2]),
            _ => return Err(format!("invalid tmux target: {}, expected session/window/pane", path).into()),
        };
        let tmux_target = format!("{}:{}.{}", session, window, pane);
        Ok(("tmux".to_string(), vec!["attach-session".to_string(), "-t".to_string(), tmux_target]))
    }
}
