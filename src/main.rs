use clap::Parser;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{self, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Monotonically increasing connection IDs (used for logging)
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Parser, Debug)]
#[command(name = "tcp-proxy", about = "A simple TCP port-forwarding proxy")]
struct Cli {
    /// Local bind address (IPv4/IPv6). Examples: 0.0.0.0:4000, [::]:4000
    #[arg(long = "listen")]
    listen: SocketAddr,

    /// Remote target address (IPv4/IPv6). Examples: 10.1.1.100:5000, [2001:db8::1]:5000
    #[arg(long = "to")]
    to: SocketAddr,
}

async fn handle_connection(
    id: u64,
    client_addr: SocketAddr,
    mut client_socket: TcpStream,
    remote_addr: SocketAddr,
) {
    // Try to connect to the remote and log with the connection ID
    let mut remote_socket = match TcpStream::connect(remote_addr).await {
        Ok(s) => {
            println!(
                "[ID:{:04}] Connected: {} ⇄ {}",
                id, client_addr, remote_addr
            );
            s
        }
        Err(e) => {
            eprintln!(
                "[ID:{:04}] Failed to connect to {} (client {}): {}",
                id, remote_addr, client_addr, e
            );
            let _ = client_socket.shutdown().await;
            return;
        }
    };

    // Pump bytes in both directions, report totals on close.
    match io::copy_bidirectional(&mut client_socket, &mut remote_socket).await {
        Ok((c_to_r, r_to_c)) => {
            println!(
                "[ID:{:04}] Closed connection: {} ⇄ {} (Client → Remote: {} B, Remote → Client: {} B)",
                id, client_addr, remote_addr, c_to_r, r_to_c
            );
        }
        Err(e) => eprintln!(
            "[ID:{:04}] Piping error ({} ⇄ {}): {}",
            id, client_addr, remote_addr, e
        ),
    }

    let _ = client_socket.shutdown().await;
    let _ = remote_socket.shutdown().await;
}

#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let listener = match TcpListener::bind(args.listen).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("Failed to bind to address {}: {}", args.listen, e);
            return;
        }
    };

    println!("Listening on {}, forwarding to {}", args.listen, args.to);

    loop {
        match listener.accept().await {
            Ok((socket, client_addr)) => {
                // Allocate a new ID for this connection.
                let id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
                println!("[ID:{:04}] Accepted connection from {}", id, client_addr);

                let to = args.to;
                tokio::spawn(async move {
                    handle_connection(id, client_addr, socket, to).await;
                });
            }
            Err(e) => {
                eprintln!("Failed to accept connection: {}", e);
            }
        }
    }
}
