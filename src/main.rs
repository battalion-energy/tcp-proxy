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
    #[arg(short, long = "pro", value_name = "LISTEN=TARGET", value_parser = parse_proxy, required = true)]
    proxies: Vec<Proxy>,
    /// Max time to establish the outbound connection (humantime, e.g., 2s, 500ms)
    #[arg(short, long = "connect-timeout", default_value = "5s", value_parser = humantime::parse_duration, value_name = "DURATION")]
    connect_timeout: Duration,
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

    for proxy in args.proxies {
        let listener = TcpListener::bind(proxy.listen)
            .await
            .context("unable to bind listener")?;

        info!(listen = %proxy.listen, to = %&proxy.target, "listening (Ctrl+C exits immediately)");

        tokio::spawn(accept_connections(
            listener,
            proxy.target,
            args.connect_timeout,
        ));
    }

    let _ = signal::ctrl_c().await;
    info!("Ctrl+C received — exiting immediately");
    Ok(())
}

async fn accept_connections(listener: TcpListener, remote: SocketAddr, connect_timeout: Duration) {
    let mut next_conn_id: u64 = 1;

    loop {
        match listener.accept().await {
            Ok((socket, client_addr)) => {
                let id = next_conn_id;
                next_conn_id += 1;
                info!(id = id, client = %client_addr,  "accepted connection");
                let span =
                    tracing::info_span!("conn", id = id, client = %client_addr, remote = %remote);
                tokio::spawn(handle_connection(socket, remote, connect_timeout).instrument(span));
            }
            Err(e) => warn!(error = %e, "failed to accept connection"),
        }
    }
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

/// One listener, one proxy
#[derive(Clone, Debug, Parser)]
struct Proxy {
    listen: SocketAddr,
    target: SocketAddr,
}

fn parse_proxy(s: &str) -> Result<Proxy, anyhow::Error> {
    let (listen, target) = s
        .split_once('=')
        .context("expected LISTEN=TARGET, e.g 127.0.0.1:5001=127.0.0.1:9000")?;

    Ok(Proxy {
        listen: listen
            .parse()
            .context("bad listen address {listen:?}: {e}")?,
        target: target
            .parse()
            .context("bad target address {target:?}: {e}")?,
    })
}
