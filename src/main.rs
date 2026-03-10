mod backends;

use std::io::Read as _;
use std::time::Duration;

use clap::{Parser, Subcommand};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

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

pub struct Target {
    pub url: String,
}

fn parse_target_url(target: &str) -> Result<(&str, &str), Box<dyn std::error::Error>> {
    let (scheme, path) = target
        .split_once("://")
        .ok_or_else(|| format!("invalid target URL: {}", target))?;
    Ok((scheme, path))
}

fn build_target_command(target: &str) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
    let (scheme, path) = parse_target_url(target)?;
    let backend = backends::backend_for_scheme(scheme)
        .ok_or_else(|| format!("unknown target scheme: {}", scheme))?;
    backend.build_command(path)
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
        (None, Some(target)) if backends::is_target_url(&target) => Commands::Attach { target },
        (None, _) => Commands::Ls,
    };

    match command {
        Commands::Ls => {
            let backends = backends::all_backends();
            let mut futures = Vec::new();
            for backend in &backends {
                futures.push(backend.list_targets());
            }
            let results = futures::future::join_all(futures).await;
            for target in results.into_iter().flatten() {
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
