use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
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
/// The subcommand picks what listeners bind to, and with it the form --proxy
/// takes: a bare port when an interface is filtering, a full address when not.
///
/// Examples:
///   tcp-proxy tailscale0 --proxy 5001=127.0.0.1:9000
///   tcp-proxy device wg0 --proxy 5001=10.1.1.10:6000 --proxy 5002=10.1.1.11:6000
///   tcp-proxy any --proxy 0.0.0.0:5000=10.1.1.10:6000
#[command(
    name = "tcp-proxy",
    version,
    about = "Forward TCP connections from each --proxy listener to its target",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    interface: Interface,
    /// Max time to establish the outbound connection (humantime, e.g., 2s, 500ms)
    #[arg(
        short,
        long = "connect-timeout",
        global = true,
        default_value = "5s",
        value_parser = humantime::parse_duration,
        value_name = "DURATION"
    )]
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
    let device = args.interface.device_name();
    let listeners = configure_listeners(&args.interface, device)?;

    for (listener, target) in listeners {
        info!(
            listen = %listener.local_addr()?,
            to = %target,
            interface = device.unwrap_or("any"),
            "listening (Ctrl+C exits immediately)"
        );

        tokio::spawn(accept_connections(listener, target, args.connect_timeout));
    }

    signal::ctrl_c().await?;
    info!("Ctrl+C received — exiting immediately");
    Ok(())
}

fn configure_listeners(
    interface: &Interface,
    device: Option<&str>,
) -> Result<Vec<(TcpListener, SocketAddr)>> {
    let pairs: Vec<(SocketAddr, SocketAddr)> = match interface {
        Interface::Any { proxies } => proxies.iter().map(|p| (p.listen, p.target)).collect(),
        Interface::Tailscale0 { proxies } | Interface::Device { proxies, .. } => proxies
            .iter()
            .map(|p| {
                (
                    SocketAddr::from((Ipv4Addr::UNSPECIFIED, p.listen)),
                    p.target,
                )
            })
            .collect(),
    };

    let mut listeners = vec![];
    for (listen, target) in pairs {
        listeners.push((bind_listener(listen, device)?, target));
    }

    Ok(listeners)
}

fn bind_listener(addr: SocketAddr, device: Option<&str>) -> Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;

    if let Some(name) = device {
        // SO_BINDTODEVICE is Linux-only, and socket2 cfg-gates it away entirely,
        // so the call has to be compiled out rather than just skipped.
        #[cfg(target_os = "linux")]
        socket
            .bind_device(Some(name.as_bytes()))
            .with_context(|| format!("binding to interface: {name}"))?;

        #[cfg(not(target_os = "linux"))]
        anyhow::bail!("interface {name} needs SO_BINDTODEVICE, which only exists on Linux");
    }

    socket
        .bind(&addr.into())
        .with_context(|| format!("unable to bind {addr}"))?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    anyhow::Ok(TcpListener::from_std(socket.into())?)
}

async fn accept_connections(listener: TcpListener, remote: SocketAddr, connect_timeout: Duration) {
    let mut next_conn_id: u64 = 1;

    loop {
        match listener.accept().await {
            Ok((socket, client_addr)) => {
                let id = next_conn_id;
                next_conn_id += 1;

                let span = tracing::info_span!("conn", id, client = %client_addr, remote = %remote);
                span.in_scope(|| info!("accepted connection"));

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

/// One listener, one target. `L` is how the listen side is written, which
/// depends on whether an interface is doing the filtering.
#[derive(Clone, Debug)]
struct Proxy<L> {
    listen: L,
    target: SocketAddr,
}

impl<L> FromStr for Proxy<L>
where
    L: FromStr,
    L::Err: std::fmt::Display,
{
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        // clap names the expected form from value_name, so this only has to
        // report what was wrong with the value.
        let (listen, target) = s.split_once('=').context("expected LISTEN=TARGET")?;

        Ok(Self {
            listen: listen
                .parse()
                .map_err(|e| anyhow!("bad listen address {listen:?}: {e}"))?,
            target: target
                .parse()
                .map_err(|e| anyhow!("bad target address {target:?}: {e}"))?,
        })
    }
}

/// What listeners bind to. Each variant carries only the proxies whose listen
/// form suits it, so the mismatched combination can't be expressed.
///
/// Interface restriction needs `SO_BINDTODEVICE`, so `tailscale0` and `device`
/// are accepted everywhere but fail at startup off Linux.
#[derive(Debug, Subcommand)]
enum Interface {
    /// Bind the given addresses directly, with no interface restriction
    Any {
        /// Forwarding rule, repeatable (e.g., 0.0.0.0:5001=127.0.0.1:9000)
        #[arg(
            short,
            long = "proxy",
            value_name = "ADDR:PORT=TARGET",
            required = true
        )]
        proxies: Vec<Proxy<SocketAddr>>,
    },
    /// Restrict listeners to the tailscale0 interface
    Tailscale0 {
        /// Forwarding rule, repeatable (e.g., 5001=127.0.0.1:9000)
        #[arg(short, long = "proxy", value_name = "PORT=TARGET", required = true)]
        proxies: Vec<Proxy<u16>>,
    },
    /// Restrict listeners to a named interface
    Device {
        /// Interface name (e.g., wg0)
        #[arg(value_name = "IFACE")]
        name: String,
        /// Forwarding rule, repeatable (e.g., 5001=127.0.0.1:9000)
        #[arg(short, long = "proxy", value_name = "PORT=TARGET", required = true)]
        proxies: Vec<Proxy<u16>>,
    },
}

impl Interface {
    fn device_name(&self) -> Option<&str> {
        match self {
            Interface::Any { .. } => None,
            Interface::Tailscale0 { .. } => Some("tailscale0"),
            Interface::Device { name, .. } => Some(name),
        }
    }
}
