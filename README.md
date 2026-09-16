# tcp-proxy

A small, async TCP port-forwarding proxy built with Rust and Tokio. Listens on one or more local ports, forwards raw bytes bidirectionally to a target, and logs through `tracing`.

## Quick Start

```bash
cargo build --release

# port 5001 on tailscale0 -> 127.0.0.1:6000
RUST_LOG=info ./target/release/tcp-proxy tailscale0 --proxy 5001=127.0.0.1:6000
```

## Usage

```text
tcp-proxy [--connect-timeout <DURATION>] <COMMAND>

  tailscale0   --proxy <PORT=TARGET>...
  device IFACE --proxy <PORT=TARGET>...
  any          --proxy <ADDR:PORT=TARGET>...
```

The subcommand decides what listeners bind to, and with it the form `--proxy` (`-p`) takes. Under `tailscale0` and `device` the listener is named by port alone and restricted to that interface, so nothing arriving elsewhere is accepted. Under `any` there is no restriction, so the listener needs a full address. `TARGET` is always `ADDR:PORT`.

Repeat `--proxy` for more than one listener. All of them share the one interface.

`--connect-timeout` (`-c`) caps the outbound connect, default `5s`, in `humantime` format (`250ms`, `2m`, `1h`).

### Examples

```bash
# two listeners on the tailnet
tcp-proxy tailscale0 -p 5001=10.1.1.10:6000 -p 5002=10.1.1.11:6000

# another interface
tcp-proxy device wg0 -p 5001=127.0.0.1:9000

# unrestricted, with a 2s connect timeout
tcp-proxy any -p 0.0.0.0:5000=10.1.1.10:6000 -c 2s
```

Local test with netcat, which needs `any` because loopback is not an interface you can restrict to usefully:

```bash
nc -lk 127.0.0.1 6000                                   # backend
tcp-proxy any -p 127.0.0.1:5001=127.0.0.1:6000          # proxy
nc 127.0.0.1 5001                                       # client
```

## Interface binding

Restriction uses `SO_BINDTODEVICE`. It needs no privileges, but it is Linux-only: `tailscale0` and `device` are accepted everywhere and fail at startup elsewhere. Use `any` there.

It is not a firewall. A local process can still reach the interface's own address; what the restriction excludes is traffic arriving on other interfaces. Who may connect over the tailnet is a Tailscale ACL question.

A bare port binds `0.0.0.0`, so IPv6 peers are refused even when the interface has a v6 address.

## Logging

`RUST_LOG` sets the level, default `info`. `RUST_LOG=tcp_proxy=debug` scopes it to this crate.

Each listener logs its bound address, target, and interface at startup. Per-connection events carry a `conn{id, client, remote}` span. Ids count per listener, so `(remote, id)` identifies a session.

## Notes

- Ctrl+C exits immediately; active connections are aborted.
- On connect timeout the client socket is closed and the attempt logged as a warning.
- No authentication, authorization, or TLS.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
