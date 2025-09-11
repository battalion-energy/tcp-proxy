use anyhow::Context;
use clap::Parser;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::task::JoinSet;
use tokio::time;
use tracing::Instrument;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
/// A simple TCP port-forwarding proxy
///
/// Address format:
/// - IPv4: A.B.C.D:PORT (e.g., 127.0.0.1:5001)
/// - IPv6: [IPv6]:PORT (e.g., [::1]:9000)
///
/// Examples:
///   tcp-proxy --listen 127.0.0.1:5001 --to 127.0.0.1:9000
///   tcp-proxy --listen 0.0.0.0:5000 --to 10.1.1.10:6000 --connect-timeout 2s
#[command(
    name = "tcp-proxy",
    version,
    about = "Forward TCP connections from --listen to --to",
    long_about = None
)]
struct Cli {
    /// Local address:port to accept client connections (e.g., 127.0.0.1:5001)
    #[arg(long = "listen", value_name = "ADDR:PORT")]
    listen: SocketAddr,
    /// Remote target address:port to forward to (e.g., 127.0.0.1:9000)
    #[arg(long = "to", value_name = "ADDR:PORT")]
    to: SocketAddr,
    /// Max time to establish the outbound connection (humantime, e.g., 2s, 500ms)
    #[arg(long = "connect-timeout", default_value = "5s", value_parser = humantime::parse_duration, value_name = "DURATION")]
    connect_timeout: Duration,
    /// Max lifetime of a proxied connection; 0s by default (disables the timeout)
    #[arg(long = "session-timeout", default_value = "0s", value_parser = humantime::parse_duration, value_name = "DURATION")]
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
        // If zero, there is no timeout and the proxying runs until the connection closes
        tokio::io::copy_bidirectional(&mut client_socket, &mut remote_socket)
            .await
            .map(Some)
    } else {
        match time::timeout(
            session_timeout,
            tokio::io::copy_bidirectional(&mut client_socket, &mut remote_socket),
        )
        .await
        {
            Ok(inner) => inner.map(Some),
            Err(_) => Ok(None),
        }
    };

    match res {
        Ok(Some((c_to_r, r_to_c))) => {
            info!(
                client_to_remote = c_to_r,
                remote_to_client = r_to_c,
                "Closed connection"
            );
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
async fn main() -> anyhow::Result<()> {
    // Initialize logging from RUST_LOG or default to info
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args = Cli::parse();
    let listener = TcpListener::bind(args.listen)
        .await
        .context("unable to bind listener")?;

    info!(listen = %args.listen, to = %args.to, "Listening (Ctrl+C to stop accepting)");

    let mut tasks: JoinSet<()> = JoinSet::new();
    let to = args.to;
    let connect_timeout = args.connect_timeout;
    let session_timeout = args.session_timeout;

    let mut next_conn_id: u64 = 1;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Ctrl+C received — stopping accept and waiting for active connections...");
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((socket, client_addr)) => {
                        let id = next_conn_id;
                        next_conn_id += 1;
                        info!(id = id, client = %client_addr, "Accepted connection");
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

    // Close listener so no further accepts happen
    drop(listener);

    while let Some(res) = tasks.join_next().await {
        if let Err(e) = res {
            error!(error = %e, "A connection task ended with an error");
        }
    }

    info!("Shutdown complete.");
    Ok(())
}
