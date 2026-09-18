use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lxh_daemon::server::DaemonServer;
use lxh_runtime::Runtime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn resolve_socket_path(args: &mut Vec<String>) -> PathBuf {
    let mut idx = None;
    for (i, arg) in args.iter().enumerate() {
        if arg == "--socket" {
            idx = Some(i);
            break;
        }
    }

    if let Some(i) = idx {
        if i + 1 >= args.len() {
            eprintln!("error: --socket requires a value");
            std::process::exit(1);
        }
        let path = args.remove(i + 1);
        args.remove(i);
        return PathBuf::from(path);
    }

    env::var("LXH_SOCKET_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp"))
                .join("linux-x11-harness.sock")
        })
}

fn pid_path(socket_path: &Path) -> PathBuf {
    let mut p = socket_path.to_path_buf();
    p.set_extension("pid");
    p
}

#[tokio::main]
async fn main() {
    let mut args: Vec<String> = env::args().collect();
    let socket_path = resolve_socket_path(&mut args);
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("mcp");

    if command == "--help" || command == "-h" {
        println!("Usage: linux-x11-harness [serve|mcp|stop|status] [--socket PATH]");
        return;
    }

    match command {
        "serve" => run_serve(socket_path).await,
        "mcp" => run_mcp(socket_path).await,
        "stop" => run_stop(socket_path).await,
        "status" => run_status(socket_path).await,
        _ => {
            eprintln!("Usage: linux-x11-harness [serve|mcp|stop|status] [--socket PATH]");
            std::process::exit(1);
        }
    }
}

async fn run_serve(socket_path: PathBuf) {
    let pid = std::process::id();
    let _ = tokio::fs::write(pid_path(&socket_path), pid.to_string()).await;

    let runtime = Arc::new(Runtime::new());
    let server = DaemonServer::new(runtime, socket_path);
    if let Err(e) = server.run().await {
        eprintln!("daemon error: {e}");
        std::process::exit(1);
    }
}

async fn run_stop(socket_path: PathBuf) {
    let pid_path = pid_path(&socket_path);
    let pid: u32 = match tokio::fs::read_to_string(&pid_path).await {
        Ok(s) => s.trim().parse().unwrap_or(0),
        Err(_) => {
            eprintln!("daemon not running");
            std::process::exit(1);
        }
    };
    if pid == 0 {
        eprintln!("daemon not running");
        std::process::exit(1);
    }
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    let _ = tokio::fs::remove_file(&pid_path).await;
    let _ = tokio::fs::remove_file(&socket_path).await;
    println!("stopped daemon {pid}");
}

async fn run_status(socket_path: PathBuf) {
    if !socket_path.exists() {
        println!("stopped");
        std::process::exit(1);
    }
    println!("running");
}

/// Start the daemon in its own session so it is reparented to init
/// immediately. Agents that kill their MCP server's whole process tree on
/// exit (e.g. OpenCode) would otherwise take the shared daemon — and every
/// display inside it — down with them.
async fn spawn_daemon_detached(socket_path: &Path) {
    let executable = env::current_exe().expect("current exe");

    let detached = tokio::process::Command::new("setsid")
        .arg("-f")
        .arg(&executable)
        .arg("serve")
        .arg("--socket")
        .arg(socket_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    if detached.is_err() || !detached.unwrap().success() {
        // setsid unavailable or failed; fall back to a direct child.
        let mut cmd = tokio::process::Command::new(&executable);
        cmd.arg("serve").arg("--socket").arg(socket_path);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.spawn().expect("spawn daemon");
    }

    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if socket_path.exists() {
            return;
        }
    }

    eprintln!("daemon failed to start");
    std::process::exit(1);
}

async fn run_mcp(socket_path: PathBuf) {
    // Connect to the running daemon; if that fails (no daemon yet, or a stale
    // socket file left behind by a killed daemon), start a fresh one. Clients
    // may sanitize the environment, so the resolved path can differ between
    // agents — never assume an existing socket is the right one.
    let mut stream = None;
    for _ in 0..4 {
        if let Ok(s) = tokio::net::UnixStream::connect(&socket_path).await {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let stream = match stream {
        Some(stream) => stream,
        None => {
            let _ = tokio::fs::remove_file(&socket_path).await;
            spawn_daemon_detached(&socket_path).await;
            tokio::net::UnixStream::connect(&socket_path)
                .await
                .expect("connect daemon")
        }
    };

    let (read_half, mut write_half) = stream.into_split();

    // Forward both directions concurrently. Awaiting the stdin->socket
    // direction before polling socket->stdout would deadlock interactive
    // clients until stdin closes.
    let to_stdout = tokio::spawn(forward(read_half, tokio::io::stdout()));
    forward(tokio::io::stdin(), &mut write_half).await;
    let _ = write_half.shutdown().await;
    let _ = to_stdout.await;
}

async fn forward<R, W>(mut reader: R, mut writer: W)
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
