mod backends;

use std::io::Read as _;
use std::time::Duration;

use clap::{Parser, Subcommand};
use dialoguer::{Select, theme::ColorfulTheme};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

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
    Ps {
        /// Only show URLs
        #[arg(short)]
        q: bool,
    },
    /// Attach to a target
    Attach {
        /// Target URL (e.g. docker://name, docker://project/service, tmux://session/window/pane)
        target: String,
    },
    /// Interactively select a target to attach to
    Interactive,
    /// Print one frame of the target's terminal output
    Screenshot {
        /// Target URL (e.g. docker://name, docker://project/service, tmux://session/window/pane)
        target: String,
        /// Terminal size as COLSxROWS (e.g. 80x24). Defaults to current terminal size.
        #[arg(long)]
        size: Option<String>,
    },
}

pub struct Target {
    pub url: String,
    pub aliases: Vec<String>,
    pub id: String,
    pub command: String,
    pub created: String,
}

fn parse_target_url(target: &str) -> Result<(&str, &str), Box<dyn std::error::Error>> {
    let (scheme, path) = target
        .split_once("://")
        .ok_or_else(|| format!("invalid target URL: {}", target))?;
    Ok((scheme, path))
}

fn backend_for_target(target: &str) -> Result<(Box<dyn backends::Backend>, &str), Box<dyn std::error::Error>> {
    let (scheme, path) = parse_target_url(target)?;
    let backend = backends::backend_for_scheme(scheme)
        .ok_or_else(|| format!("unknown target scheme: {}", scheme))?;
    Ok((backend, path))
}

fn attach(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, path) = backend_for_target(target)?;
    backend.attach(path)
}

fn parse_size(size: Option<&str>) -> Result<(u16, u16), Box<dyn std::error::Error>> {
    match size {
        Some(s) => {
            let (cols, rows) = s
                .split_once('x')
                .ok_or_else(|| format!("invalid size '{}', expected COLSxROWS (e.g. 80x24)", s))?;
            Ok((cols.parse()?, rows.parse()?))
        }
        None => Ok(terminal_size::terminal_size()
            .map(|(w, h)| (w.0, h.0))
            .unwrap_or((80, 24))),
    }
}

pub fn pty_screenshot(program: &str, args: &[String], size: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let (cols, rows) = parse_size(size)?;

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(program);
    for arg in args {
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

    let screen = parser.screen();
    let mut stdout = std::io::stdout();
    for (i, row) in screen.rows_formatted(0, cols).enumerate() {
        if i >= rows as usize {
            break;
        }
        std::io::Write::write_all(&mut stdout, &row)?;
        std::io::Write::write_all(&mut stdout, b"\x1b[m")?;
        if i < (rows as usize).saturating_sub(1) {
            std::io::Write::write_all(&mut stdout, b"\r\n")?;
        }
    }
    std::io::Write::write_all(&mut stdout, b"\x1b[?25h")?;
    println!();

    Ok(())
}

async fn list_all_targets() -> Result<Vec<Target>, Box<dyn std::error::Error>> {
    let backends = backends::all_backends();
    let mut futures = Vec::new();
    for backend in &backends {
        futures.push(backend.list_targets());
    }
    let results = futures::future::join_all(futures).await;
    let mut targets = Vec::new();
    for result in results {
        match result {
            Ok(t) => targets.extend(t),
            Err(e) => {
                if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    if io_err.kind() == std::io::ErrorKind::NotFound {
                        continue;
                    }
                }
                eprintln!("warning: {}", e);
            }
        }
    }
    Ok(targets)
}

fn screenshot(target: &str, size: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, path) = backend_for_target(target)?;
    backend.screenshot(path, size)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let command = match (cli.command, cli.target) {
        (Some(cmd), _) => cmd,
        (None, Some(target)) if backends::is_target_url(&target) => Commands::Attach { target },
        (None, _) => Commands::Ps { q: false },
    };

    match command {
        Commands::Ps { q } => {
            let targets = match list_all_targets().await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            };

            if q {
                for target in &targets {
                    println!("{}", target.url);
                }
            } else {
                println!("{:<24} {:<20} {:<20} {}", "ID", "COMMAND", "CREATED", "NAME");
                for target in &targets {
                    println!("{:<24} {:<20} {:<20} {}", target.id, target.command, target.created, target.url);
                }
            }
        }
        Commands::Interactive => {
            let targets = match list_all_targets().await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            };

            if targets.is_empty() {
                eprintln!("No attachable targets found.");
                std::process::exit(1);
            }

            let items: Vec<String> = targets
                .iter()
                .map(|t| format!("{} ({})", t.url, t.command))
                .collect();

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select a target to attach to")
                .items(&items)
                .default(0)
                .interact_opt();

            match selection {
                Ok(Some(index)) => {
                    if let Err(e) = attach(&targets[index].url) {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Attach { target } => {
            if let Err(e) = attach(&target) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Screenshot { target, size } => {
            if let Err(e) = screenshot(&target, size.as_deref()) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
