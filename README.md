# tcp-proxy

A small, async TCP port-forwarding proxy built with Rust and Tokio. Listens on one or more local ports, forwards raw bytes bidirectionally to a target, and logs through `tracing`.

## Quick Start

```bash
cargo build --release

# port 5001 on tailscale0 -> 127.0.0.1:6000
RUST_LOG=info ./target/release/tcp-proxy --proxy tailscale0:5001=127.0.0.1:6000
```

## Usage

```text
tcp-proxy --proxy <SOURCE=TARGET>... [--connect-timeout <DURATION>]
```

`--proxy` (`-p`) takes a source and a target, and repeats for more than one listener. `TARGET` is always `ADDR:PORT`. `SOURCE` is one of:

- `IFACE:PORT` (e.g. `tailscale0:5001`), which binds the port and restricts the listener to that interface. Traffic arriving on any other interface is not accepted.
- `ADDR:PORT` (e.g. `0.0.0.0:5001`, `127.0.0.1:5001`), which binds that address with no interface restriction.

The two are told apart by whether the source parses as an address, so each `--proxy` picks its own form and different listeners can use different interfaces.

`--connect-timeout` (`-c`) caps the outbound connect, default `5s`, in `humantime` format (`250ms`, `2m`, `1h`).

### Examples

```bash
# two listeners on the tailnet, different backends
tcp-proxy -p tailscale0:5001=10.1.1.10:6000 -p tailscale0:5002=10.1.1.11:6000

# one per interface in a single process
tcp-proxy -p tailscale0:5001=127.0.0.1:9000 -p wg0:5001=127.0.0.1:9000

# unrestricted, with a 2s connect timeout
tcp-proxy -p 0.0.0.0:5000=10.1.1.10:6000 -c 2s
```

Local test with netcat:

```bash
nc -lk 127.0.0.1 6000                          # backend
tcp-proxy -p 127.0.0.1:5001=127.0.0.1:6000     # proxy
nc 127.0.0.1 5001                              # client
```

`lo` works as an interface if you want to exercise the restricted path locally: `-p lo:5001=127.0.0.1:6000` is reachable on `127.0.0.1` and refused on every other address.

## Interface binding

Restriction uses `SO_BINDTODEVICE`. It needs no privileges, but it is Linux-only, so the `IFACE:PORT` form fails at startup elsewhere. A missing interface fails with `No such device (os error 19)`.

It is not a firewall. A local process can still reach the interface's own address; what the restriction excludes is traffic arriving on other interfaces. Who may connect over a tailnet is a Tailscale ACL question.

The restricted form binds `0.0.0.0`, so IPv6 peers are refused even when the interface has a v6 address. Tailscale gives every node both, so a peer connecting by MagicDNS name may try v6 first and fall back.

## Logging

`RUST_LOG` sets the level, default `info`. `RUST_LOG=tcp_proxy=debug` scopes it to this crate.

Each listener logs its bound address and target at startup. Per-connection events carry a `conn{id, client, remote}` span. Ids count per listener, so `(remote, id)` identifies a session.

The startup line reports the resolved bind address, not the interface, so a restricted listener logs `0.0.0.0:5001` with nothing naming the device.

## Notes

- Ctrl+C exits immediately; active connections are aborted.
- On connect timeout the client socket is closed and the attempt logged as a warning.
- No authentication, authorization, or TLS.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
