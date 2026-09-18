use anyhow::{Context, Result, anyhow};
use clap::Parser;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::signal;
use tokio::time;
use tracing::Instrument;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
/// A simple TCP port-forwarding proxy
///
/// Each --proxy gives a source and a target. An interface name plus a port
/// restricts that listener to the interface; a full address binds the address
/// with no restriction.
///
/// Examples:
///   tcp-proxy --proxy tailscale0:5001=127.0.0.1:9000
///   tcp-proxy --proxy wg0:5001=10.1.1.10:6000 --proxy wg0:5002=10.1.1.11:6000
///   tcp-proxy --proxy 0.0.0.0:5000=10.1.1.10:6000
#[command(
    name = "tcp-proxy",
    version,
    about = "Forward TCP connections from each --proxy listener to its target",
    long_about = None
)]
struct Cli {
    /// Forwarding rule, repeatable (e.g., tailscale0:5001=127.0.0.1:9000)
    #[arg(short, long = "proxy", value_name = "SOURCE=TARGET", required = true)]
    proxies: Vec<Proxy>,
    /// Max time to establish the outbound connection (humantime, e.g., 2s, 500ms)
    #[arg(
        short,
        long = "connect-timeout",
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

    let listeners = configure_listeners(args.proxies)?;

    for (proxy, listener) in listeners {
        info!(
            source = %proxy.source,
            dest = %proxy.destination,
            "listening"
        );

        tokio::spawn(accept_connections(
            listener,
            proxy.destination,
            args.connect_timeout,
        ));
    }

    signal::ctrl_c().await?;
    info!("Ctrl+C received — exiting immediately");
    Ok(())
}

fn configure_listeners(proxies: Vec<Proxy>) -> Result<Vec<(Proxy, TcpListener)>> {
    let mut listeners = Vec::new();
    for proxy in proxies {
        let listener = bind_listener(&proxy.source)
            .with_context(|| format!("unable to listen on {}", proxy.source))?;
        listeners.push((proxy, listener));
    }
    Ok(listeners)
}

fn bind_listener(source: &Source) -> Result<TcpListener> {
    let addr = match source {
        Source::Device { port, .. } => SocketAddr::from((Ipv4Addr::UNSPECIFIED, *port)),
        Source::Addr(addr) => *addr,
    };

    let socket = if addr.is_ipv6() {
        TcpSocket::new_v6()?
    } else {
        TcpSocket::new_v4()?
    };
    // TcpListener::bind sets this for us, TcpSocket does not. Without it a
    // restart fails to bind while old connections sit in TIME_WAIT.
    socket.set_reuseaddr(true)?;

    #[cfg(target_os = "linux")]
    if let Source::Device { device, .. } = source {
        socket.bind_device(Some(device.as_bytes()))?;
    }
    socket.bind(addr)?;

    anyhow::Ok(socket.listen(1024)?)
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
            Err(e) => {
                warn!(error = %e, "failed to accept connection");
                // Errors like EMFILE persist until something closes, so
                // retrying immediately would spin and flood the log.
                time::sleep(Duration::from_millis(100)).await;
            }
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

/// One listener and the target its connections are forwarded to.
#[derive(Clone, Debug)]
struct Proxy {
    source: Source,
    destination: SocketAddr,
}

/// Where a listener binds.
///
/// `Device` restricts it to one interface with `SO_BINDTODEVICE`, which is
/// Linux-only, and binds the unspecified address so the device does the
/// filtering. `Addr` binds exactly what it is given.
#[derive(Clone, Debug)]
enum Source {
    Device { device: String, port: u16 },
    Addr(SocketAddr),
}

/// Formats the same way `FromStr` accepts it, e.g. `tailscale0:5001` or
/// `0.0.0.0:5000`.
impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device { device, port } => write!(f, "{device}:{port}"),
            Self::Addr(addr) => write!(f, "{addr}"),
        }
    }
}

impl FromStr for Proxy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        // clap names the expected form from value_name, so this only has to
        // report what was wrong with the value.
        let (source, destination) = s.split_once('=').context("expected LISTEN=TARGET")?;

        Ok(Self {
            source: source
                .parse()
                .map_err(|e| anyhow!("bad source address {source:?}: {e}"))?,
            destination: destination
                .parse()
                .map_err(|e| anyhow!("bad destination address {destination:?}: {e}"))?,
        })
    }
}

impl FromStr for Source {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.parse::<SocketAddr>() {
            Ok(socket) => Ok(Self::Addr(socket)),
            Err(_) => {
                let (device, port) = s.split_once(':').context("expected <DEVICE|IP>:<PORT>")?;
                if device.is_empty() {
                    return Err(anyhow!("device name must be present"));
                }

                let port = port.parse()?;

                if cfg!(not(target_os = "linux")) {
                    anyhow::bail!(
                        "interface {device} needs SO_BINDTODEVICE, which only exists on Linux"
                    );
                }

                Ok(Self::Device {
                    device: device.to_string(),
                    port,
                })
            }
        }
    }
}
