use anyhow::Context;
use clap::Parser;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::time;
use tracing::Instrument;
use tracing::{info, warn};
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
}
async fn handle_connection(
    client_socket: TcpStream,
    remote_addr: SocketAddr,
    connect_timeout: Duration,
) {
    async fn handle_connection_inner(
        mut client_socket: TcpStream,
        remote_addr: SocketAddr,
        connect_timeout: Duration,
    ) -> anyhow::Result<(u64, u64)> {
        let mut remote_socket = time::timeout(connect_timeout, TcpStream::connect(remote_addr))
            .await
            .context("connect timed out")?
            .context("failed to connect to remote")?;

        let stats = tokio::io::copy_bidirectional(&mut client_socket, &mut remote_socket)
            .await
            .context("proxying data")?;

        Ok(stats)
    }

    match handle_connection_inner(client_socket, remote_addr, connect_timeout).await {
        Ok((c_to_r, r_to_c)) => {
            info!(
                client_to_remote = c_to_r,
                remote_to_client = r_to_c,
                "closed connection"
            );
        }
        Err(err) => {
            warn!("session error: {err}");
        }
    }
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

    let to = args.to;
    let connect_timeout = args.connect_timeout;

    let mut next_conn_id: u64 = 1;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("ctrl+c received — stopping accepting");
                return Ok(());
            }
            res = listener.accept() => {
                match res {
                    Ok((socket, client_addr)) => {
                        let id = next_conn_id;
                        next_conn_id += 1;
                        info!(id = id, client = %client_addr, "Accepted connection");
                        let span = tracing::info_span!("conn", id = id, client = %client_addr, remote = %to);
                        tokio::spawn(handle_connection(socket, to, connect_timeout).instrument(span));
                    }
                    Err(e) => warn!(error = %e, "Failed to accept connection"),
                }
            }
        }
    }
}
