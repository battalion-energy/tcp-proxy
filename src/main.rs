use clap::Parser;
use std::io as stdio;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::task::JoinSet;
use tokio::time;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing::Instrument;

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Parser, Debug)]
#[command(name = "tcp-proxy", about = "A simple TCP port-forwarding proxy")]
struct Cli {
    #[arg(long = "listen")]
    listen: SocketAddr,
    #[arg(long = "to")]
    to: SocketAddr,
    #[arg(long = "connect-timeout", default_value = "5s", value_parser = humantime::parse_duration)]
    connect_timeout: Duration,
    #[arg(long = "session-timeout", default_value = "0s", value_parser = humantime::parse_duration)]
    session_timeout: Duration,
}

async fn handle_connection(
    mut client_socket: TcpStream,
    remote_addr: SocketAddr,
    connect_timeout: Duration,
    session_timeout: Duration,
) {
    let connect = time::timeout(connect_timeout, TcpStream::connect(remote_addr)).await;
    let mut remote_socket = match connect {
        Ok(Ok(s)) => {
            info!("Connected");
            s
        }
        Ok(Err(e)) => {
            error!(error = %e, "Failed to connect");
            let _ = client_socket.shutdown().await;
            return;
        }
        Err(_) => {
            error!(timeout = ?connect_timeout, "Connect timeout");
            let _ = client_socket.shutdown().await;
            return;
        }
    };

    let res = if session_timeout.is_zero() {
        tokio::io::copy_bidirectional(&mut client_socket, &mut remote_socket).await.map(Some)
    } else {
        match time::timeout(session_timeout, tokio::io::copy_bidirectional(&mut client_socket, &mut remote_socket)).await {
            Ok(inner) => inner.map(Some),
            Err(_) => Ok(None),
        }
    };

    match res {
        Ok(Some((c_to_r, r_to_c))) => {
            info!(client_to_remote = c_to_r, remote_to_client = r_to_c, "Closed connection");
        }
        Ok(None) => {
            warn!(timeout = ?session_timeout, "Session timeout");
        }
        Err(e) => {
            error!(error = %e, "Piping error");
        }
    }

    let _ = client_socket.shutdown().await;
    let _ = remote_socket.shutdown().await;
}

#[tokio::main]
async fn main() -> stdio::Result<()> {
    // Initialize logging with env filter. Example: RUST_LOG=tcp_proxy=debug,info
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let args = Cli::parse();
    let listener = TcpListener::bind(args.listen).await?;
    info!(listen = %args.listen, to = %args.to, "Listening (Ctrl+C to stop accepting)");

    let mut tasks: JoinSet<()> = JoinSet::new();
    let to = args.to;
    let connect_timeout = args.connect_timeout;
    let session_timeout = args.session_timeout;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Ctrl+C received — stopping accept and waiting for active connections...");
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((socket, client_addr)) => {
                        let id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
                        info!(id = id, client = %client_addr, "Accepted connection");
                        let to = to;
                        let span = tracing::info_span!("conn", id = id, client = %client_addr, remote = %to);
                        tasks.spawn(async move {
                            handle_connection(socket, to, connect_timeout, session_timeout).await;
                        }.instrument(span));
                    }
                    Err(e) => warn!(error = %e, "Failed to accept connection"),
                }
            }
        }
    }

    drop(listener);

    while let Some(res) = tasks.join_next().await {
        if let Err(e) = res {
            error!(error = %e, "A connection task ended with an error");
        }
    }

    info!("Shutdown complete.");
    Ok(())
}
