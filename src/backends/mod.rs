pub mod docker;
pub mod keys;
pub mod tmux;

use std::future::Future;
use std::pin::Pin;

use crate::Target;

pub trait Backend: Send {
    fn scheme(&self) -> &'static str;
    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Target>, Box<dyn std::error::Error + Send + Sync>>> + Send + '_>>;
    fn build_command(&self, path: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>>;

    fn attach(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::process::CommandExt;

        let (program, args) = self.build_command(path)?;
        let err = std::process::Command::new(&program)
            .args(&args)
            .exec();
        Err(err.into())
    }

    fn screenshot(&self, path: &str, size: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let (program, args) = self.build_command(path)?;
        crate::pty_screenshot(&program, &args, size)
    }

    fn send_keys(&self, _path: &str, _keys: &[String]) -> Result<(), Box<dyn std::error::Error>> {
        Err(format!("{}:// backend does not support send-keys", self.scheme()).into())
    }

    fn send_text(&self, _path: &str, _text: &str) -> Result<(), Box<dyn std::error::Error>> {
        Err(format!("{}:// backend does not support send-text", self.scheme()).into())
    }
}

pub fn is_target_url(s: &str) -> bool {
    all_backends().iter().any(|b| s.starts_with(&format!("{}://", b.scheme())))
}

pub fn backend_for_scheme(scheme: &str) -> Option<Box<dyn Backend>> {
    match scheme {
        "docker" => Some(Box::new(docker::DockerBackend)),
        "tmux" => Some(Box::new(tmux::TmuxBackend)),
        _ => None,
    }
}

pub fn all_backends() -> Vec<Box<dyn Backend>> {
    vec![
        Box::new(docker::DockerBackend),
        Box::new(tmux::TmuxBackend),
    ]
}
