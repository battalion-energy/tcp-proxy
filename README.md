# tcp-proxy

A small, async TCP port-forwarding proxy built with Rust and Tokio. It listens on one or more local ports and forwards raw bytes bidirectionally to a target address/port. Listeners are restricted to a single network interface by default. Includes structured logging via `tracing`.

## Quick Start

```bash
# Build
cargo build --release

# Run (info logs by default)
RUST_LOG=info ./target/release/tcp-proxy --proxy 5000=127.0.0.1:6000
```

That listens on port 5000 of `tailscale0` and forwards to `127.0.0.1:6000`. Nothing arriving on any other interface is accepted.

## Usage

```text
tcp-proxy --proxy <LISTEN=TARGET>... \
          [--interface <IFACE|any>] \
          [--connect-timeout <DURATION>]
```

- `--proxy <LISTEN=TARGET>` (`-p`): forwarding rule, repeat the flag for more than one. `TARGET` is always `ADDR:PORT`. `LISTEN` is either:
  - `PORT` (e.g. `5001`), bound on every address of the chosen interface. Requires an interface, so it is rejected with `--interface any`.
- `ADDR:PORT` (e.g. `127.0.0.1:5001`, `[::1]:5001`), bound on that address. Always accepted.
- `--interface <IFACE|any>` (`-i`): interface to restrict listeners to. Defaults to `tailscale0`. Use `any` for no restriction, which binds the listen address as given.
- `--connect-timeout <DURATION>` (`-c`): max time to establish the outbound connection (default: `5s`).

Durations use `humantime` format, e.g., `250ms`, `10s`, `2m`, `1h`.

### Examples

- Two listeners on the tailnet, different backends:

  ```bash
  RUST_LOG=info tcp-proxy --proxy 5001=10.1.1.10:6000 --proxy 5002=10.1.1.11:6000
  ```

- A different interface:

  ```bash
  tcp-proxy --interface wg0 --proxy 5001=127.0.0.1:9000
  ```

- No interface restriction, listening on all addresses:

  ```bash
  tcp-proxy --interface any --proxy 0.0.0.0:5000=10.1.1.10:6000
  ```

- With a 2s connect timeout:

  ```bash
  tcp-proxy --proxy 5000=127.0.0.1:6000 --connect-timeout 2s
  ```

- Quick local test with netcat, which needs `--interface any` since loopback is not the default interface:

  ```bash
  # Terminal A: echo server on 6000
  nc -lk 127.0.0.1 6000

  # Terminal B: run proxy 5001 -> 6000
  tcp-proxy --interface any --proxy 127.0.0.1:5001=127.0.0.1:6000

  # Terminal C: connect to proxy and type
  nc 127.0.0.1 5001
  ```

## Interface binding

Restriction uses `SO_BINDTODEVICE`, which is Linux-only. On other platforms the proxy still runs, but asking for an interface fails at startup; use `--interface any` there.

Two things it does not do. It is not a firewall: a local process can still reach the interface's own address. And the bind address and the interface are independent, so `--proxy 127.0.0.1:5001=...` with the default interface binds loopback *and* restricts to `tailscale0`, which nothing can reach. The startup log reports both so this is visible.

## Logging

- Uses `tracing` with environment-based filtering. Default level is `info`.
- Control verbosity with `RUST_LOG`:
  - `RUST_LOG=warn tcp-proxy ...` (only warnings and errors)
  - `RUST_LOG=info tcp-proxy ...` (normal output; default)
  - `RUST_LOG=tcp_proxy=debug tcp-proxy ...` (enable debug for this crate only)
- Each listener logs its bound address, target, and interface at startup.
- Connection context: logs emitted while handling a connection are prefixed with a span like `conn{id=..., client=..., remote=...}`. Connection ids count per listener, so `(remote, id)` identifies a session.

## Notes

- On Ctrl+C, the proxy exits immediately; active connections are aborted.
- On connect timeout, the client socket is closed and the attempt is logged as a warning.
- No authentication, authorization, or TLS.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
