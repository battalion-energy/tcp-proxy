use anyhow::{Context, Result, anyhow};
use clap::Parser;
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
    ///
    #[arg(short, long = "interface", default_value = "tailscale0", value_name = "IFACE|any")]
    interface: Interface,
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

    let listeners = configure_listeners(args.proxies, &args.interface)?;

    for (listener, target) in listeners {
        info!(
            listen = %listener.local_addr()?,
            to = %target,
            interface = args.interface.device_name().unwrap_or("any"),
            "listening (Ctrl+C exits immediately)"
        );

        tokio::spawn(accept_connections(listener, target, args.connect_timeout));
    }

    let _ = signal::ctrl_c().await;
    info!("Ctrl+C received — exiting immediately");
    Ok(())
}

fn configure_listeners(
    proxies: Vec<Proxy>,
    interface: &Interface,
) -> Result<Vec<(TcpListener, SocketAddr)>> {
    let mut listeners = vec![];

    for proxy in proxies {
        let listener = bind_listener(proxy.listen, interface)?;
        listeners.push((listener, proxy.target))
    }

    Ok(listeners)
}

fn bind_listener(addr: ProxyListener, interface: &Interface) -> Result<TcpListener> {
    let addr = match addr {
        ProxyListener::Port(port) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        ProxyListener::Address(socket_addr) => socket_addr,
    };

    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    
    if let Some(name) = interface.device_name() {
        // SO_BINDTODEVICE is Linux-only, and socket2 cfg-gates it away entirely,
        // so the call has to be compiled out rather than just skipped.
        #[cfg(target_os = "linux")]
        socket.bind_device(Some(name.as_bytes()))?;

        #[cfg(not(target_os = "linux"))]
        anyhow::bail!(
            "--interface {name} needs SO_BINDTODEVICE, which only exists on Linux; use \"any\""
        );
    };

    socket.bind(&addr.into())?;
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

/// One listener, one proxy
#[derive(Clone, Debug)]
struct Proxy {
    listen: ProxyListener,
    target: SocketAddr,
}

#[derive(Clone, Debug)]
pub enum ProxyListener {
    Port(u16),
    Address(SocketAddr),
}

impl FromStr for ProxyListener {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(Self::Address(addr))
        };

        Ok(Self::Port(s.parse::<u16>()?))
    }
}

fn parse_proxy(s: &str) -> Result<Proxy, anyhow::Error> {
    let (listen, target) = s
        .split_once('=')
        .context("expected LISTEN=TARGET, e.g 127.0.0.1:5001=127.0.0.1:9000")?;

    Ok(Proxy {
        listen: listen
            .parse()
            .map_err(|e| anyhow!("bad listen address {listen:?}: {e}"))?,
        target: target
            .parse()
            .map_err(|e| anyhow!("bad target address {target:?}: {e}"))?,
    })
}

#[derive(Clone, Debug)]
pub enum Interface {
    Any,
    Tailscale0,
    Device(String),
}

impl Interface {
    fn device_name(&self) -> Option<&str> {
        match self {
            Interface::Any => None,
            Interface::Tailscale0 => Some("tailscale0"),
            Interface::Device(name) => Some(name),
        }
    }
}

impl FromStr for Interface {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "any" => Ok(Self::Any),
            "tailscale0" => Ok(Self::Tailscale0),
            name => Ok(Self::Device(name.to_string()))
        }
    }
}

impl std::fmt::Display for Interface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Interface::Any => "any",
            Interface::Tailscale0 => "tailscale0",
            Interface::Device(device) => device,
        };
        f.write_str(str)
    }
}
