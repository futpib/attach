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
            "new-session", "-d", "-P", "-F", "#{session_id};;#{pane_id}",
            "sh", "-c", placeholder_script, "--", &pane_id,
        ])?;
        let (temp_session_id, placeholder_pane_id) = temp_session
            .split_once(";;")
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

fn translate_key(key: &str) -> String {
    // Handle modifier+key combos: ctrl+x → C-x, alt+x → M-x, shift+x → S-x
    // Support both xdotool style (ctrl+x) and chained modifiers (ctrl+shift+x)
    if let Some(pos) = key.rfind('+') {
        let (modifiers_str, base) = key.split_at(pos);
        let base = &base[1..]; // skip the '+'
        let translated_base = translate_key(base);

        let mut prefix = String::new();
        for modifier in modifiers_str.split('+') {
            match modifier.to_lowercase().as_str() {
                "ctrl" | "control" => prefix.push_str("C-"),
                "alt" | "meta" => prefix.push_str("M-"),
                "shift" => prefix.push_str("S-"),
                "super" => prefix.push_str("S-"), // tmux has no super, map to shift
                _ => {
                    // Not a known modifier, return as-is
                    return key.to_string();
                }
            }
        }
        return format!("{}{}", prefix, translated_base);
    }

    // Translate xdotool key names to tmux key names
    match key {
        "Return" | "KP_Enter" => "Enter".to_string(),
        "BackSpace" => "BSpace".to_string(),
        "Delete" | "KP_Delete" => "DC".to_string(),
        "Page_Up" | "Prior" => "PPage".to_string(),
        "Page_Down" | "Next" => "NPage".to_string(),
        "space" => "Space".to_string(),
        "Tab" | "ISO_Left_Tab" => "Tab".to_string(),
        "Escape" => "Escape".to_string(),
        "Home" => "Home".to_string(),
        "End" => "End".to_string(),
        "Left" => "Left".to_string(),
        "Right" => "Right".to_string(),
        "Up" => "Up".to_string(),
        "Down" => "Down".to_string(),
        "Insert" => "IC".to_string(),
        other => other.to_string(),
    }
}

impl Backend for TmuxBackend {
    fn scheme(&self) -> &'static str {
        "tmux"
    }

    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Target>, Box<dyn std::error::Error + Send + Sync>>> + Send + '_>> {
        Box::pin(async {
            let output = Command::new("tmux")
                .args([
                    "list-panes", "-a", "-F",
                    "#{pane_id};;#{pane_current_command};;#{session_created};;#{session_name}/#{window_name}/#{pane_index}",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("no server running")
                    || stderr.contains("error connecting")
                    || stderr.contains("no current client")
                {
                    return Ok(Vec::new());
                }
                return Err(format!("tmux list-panes failed: {}", stderr.trim()).into());
            }

            let stdout = String::from_utf8_lossy(&output.stdout);

            let result = stdout
                .lines()
                .filter(|line| !line.is_empty())
                .filter_map(|line| {
                    let mut parts = line.splitn(4, ";;");
                    let pane_id = parts.next()?;
                    let command = parts.next()?;
                    let created_ts = parts.next()?;
                    let friendly = parts.next()?;
                    let created = created_ts.parse::<i64>().ok()
                        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                        .map(|dt| chrono_humanize::HumanTime::from(dt).to_string())
                        .unwrap_or_default();
                    Some(Target {
                        url: format!("tmux://{}", friendly),
                        aliases: vec![format!("tmux://{}", pane_id)],
                        id: format!("tmux://{}", pane_id),
                        command: command.to_string(),
                        created,
                    })
                })
                .collect();
            Ok(result)
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

    fn send_keys(&self, path: &str, keys: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        let resolved = resolve_target(path)?;
        let translated: Vec<String> = keys.iter().map(|k| translate_key(k)).collect();
        let mut args: Vec<&str> = vec!["send-keys", "-t", &resolved.target];
        for key in &translated {
            args.push(key);
        }
        tmux_run(&args)?;
        Ok(())
    }

    fn send_text(&self, path: &str, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        let resolved = resolve_target(path)?;
        tmux_run(&["send-keys", "-t", &resolved.target, "-l", text])?;
        Ok(())
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
