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

enum TargetKind {
    Session,
    Window,
    Pane,
}

struct ResolvedTarget {
    target: String,
    kind: TargetKind,
}

fn resolve_target(path: &str) -> Result<ResolvedTarget, Box<dyn std::error::Error>> {
    if is_tmux_id(path) {
        let kind = match path.chars().next() {
            Some('$') => TargetKind::Session,
            Some('@') => TargetKind::Window,
            Some('%') => TargetKind::Pane,
            _ => unreachable!(),
        };
        return Ok(ResolvedTarget { target: path.to_string(), kind });
    }

    let parts: Vec<&str> = path.splitn(3, '/').collect();
    match parts.len() {
        1 => Ok(ResolvedTarget { target: parts[0].to_string(), kind: TargetKind::Session }),
        2 => Ok(ResolvedTarget { target: format!("{}:{}", parts[0], parts[1]), kind: TargetKind::Window }),
        3 => Ok(ResolvedTarget { target: format!("{}:{}.{}", parts[0], parts[1], parts[2]), kind: TargetKind::Pane }),
        _ => Err(format!("invalid tmux target: {}", path).into()),
    }
}

fn tmux_run(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("tmux")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tmux {:?} failed: {}", args, stderr.trim()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

struct PaneIsolation {
    pane_id: String,
    temp_session_id: String,
    placeholder_pane_id: String,
}

impl PaneIsolation {
    fn setup(target: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let pane_id = tmux_run(&[
            "display-message", "-t", target, "-p", "#{pane_id}",
        ])?;

        let placeholder_script = concat!(
            "printf '\\033[2m'; ",
            "echo 'Pane is attached elsewhere.'; ",
            "echo ''; ",
            "echo '  Enter - swap back'; ",
            "echo '  Esc/q - quit'; ",
            "printf '\\033[0m'; ",
            "while true; do ",
            "  IFS= read -rsn1 key; ",
            "  case \"$key\" in ",
            "    '') tmux swap-pane -s \"$1\" -t \"$TMUX_PANE\"; break;; ",
            "    q) break;; ",
            "    $'\\x1b') break;; ",
            "  esac; ",
            "done",
        );

        let temp_session = tmux_run(&[
            "new-session", "-d", "-P", "-F", "#{session_id}\t#{pane_id}",
            "sh", "-c", placeholder_script, "--", &pane_id,
        ])?;
        let (temp_session_id, placeholder_pane_id) = temp_session
            .split_once('\t')
            .ok_or("failed to parse temp session output")?;
        let temp_session_id = temp_session_id.to_string();
        let placeholder_pane_id = placeholder_pane_id.to_string();

        tmux_run(&[
            "swap-pane", "-s", &pane_id, "-t", &placeholder_pane_id,
        ])?;

        Ok(PaneIsolation { pane_id, temp_session_id, placeholder_pane_id })
    }

    fn teardown(&self) {
        let placeholder_alive = tmux_run(&[
            "display-message", "-t", &self.placeholder_pane_id, "-p", "#{pane_id}",
        ]).is_ok();

        if placeholder_alive {
            let _ = tmux_run(&[
                "swap-pane", "-s", &self.pane_id, "-t", &self.placeholder_pane_id,
            ]);
        }

        let _ = tmux_run(&["kill-session", "-t", &self.temp_session_id]);
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
        let resolved = resolve_target(path)?;
        Ok(("tmux".to_string(), vec!["attach-session".to_string(), "-t".to_string(), resolved.target]))
    }

    fn attach(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let resolved = resolve_target(path)?;

        if !matches!(resolved.kind, TargetKind::Pane) {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new("tmux")
                .args(["attach-session", "-t", &resolved.target])
                .exec();
            return Err(err.into());
        }

        let isolation = PaneIsolation::setup(&resolved.target)?;

        let status = std::process::Command::new("tmux")
            .args(["attach-session", "-t", &isolation.temp_session_id])
            .status();

        isolation.teardown();

        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("tmux attach exited with {}", s).into()),
            Err(e) => Err(e.into()),
        }
    }

    fn screenshot(&self, path: &str, size: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let resolved = resolve_target(path)?;

        if !matches!(resolved.kind, TargetKind::Pane) {
            let (program, args) = self.build_command(path)?;
            return crate::pty_screenshot(&program, &args, size);
        }

        let isolation = PaneIsolation::setup(&resolved.target)?;

        let result = crate::pty_screenshot(
            "tmux",
            &[
                "attach-session".to_string(),
                "-t".to_string(),
                isolation.temp_session_id.clone(),
            ],
            size,
        );

        isolation.teardown();

        result
    }
}
