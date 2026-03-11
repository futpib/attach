pub mod docker;
pub mod tmux;

use std::future::Future;
use std::pin::Pin;

use crate::Target;

pub trait Backend: Send {
    fn scheme(&self) -> &'static str;
    fn list_targets(&self) -> Pin<Box<dyn Future<Output = Vec<Target>> + Send + '_>>;
    fn build_command(&self, path: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>>;

    fn attach(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::process::CommandExt;

        let (program, args) = self.build_command(path)?;
        let err = std::process::Command::new(&program)
            .args(&args)
            .exec();
        Err(err.into())
    }

    fn screenshot(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (program, args) = self.build_command(path)?;
        crate::pty_screenshot(&program, &args)
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
