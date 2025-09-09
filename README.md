# tcp-proxy

A small, async TCP port‑forwarding proxy built with Rust and Tokio. It listens on a local address/port and forwards raw bytes bidirectionally to a target address/port. Includes graceful shutdown and structured logging via `tracing`.

## Quick Start

```bash
# Build
cargo build --release

# Run (info logs by default)
RUST_LOG=info ./target/release/tcp-proxy \
  --listen 127.0.0.1:5000 \
  --to 127.0.0.1:6000
```

## Usage

```text
tcp-proxy --listen <ADDR:PORT> --to <ADDR:PORT> \
          [--connect-timeout <DURATION>] [--session-timeout <DURATION>]
```

- `--listen <ADDR:PORT>`: Local address:port to accept client connections (e.g., `0.0.0.0:5000`).
- `--to <ADDR:PORT>`: Remote target address:port to forward to (e.g., `10.1.1.10:6000`).
- `--connect-timeout <DURATION>`: Max time to establish the outbound connection (default: `5s`).
- `--session-timeout <DURATION>`: Max lifetime of a connection; `0s` disables (default: `0s`).

Durations use `humantime` format, e.g., `250ms`, `10s`, `2m`, `1h`.

### Examples

- Forward local port 5000 to 10.1.1.10:6000:
  ```bash
  RUST_LOG=info tcp-proxy --listen 0.0.0.0:5000 --to 10.1.1.10:6000
  ```

- With a 2s connect timeout and 30s session timeout:
  ```bash
  tcp-proxy --listen 127.0.0.1:5000 --to 127.0.0.1:6000 \
    --connect-timeout 2s --session-timeout 30s
  ```

- Quick local test with netcat:
  ```bash
  # Terminal A: echo server on 6000
  nc -lk 127.0.0.1 6000

  # Terminal B: run proxy 5001 -> 6000
  tcp-proxy --listen 127.0.0.1:5001 --to 127.0.0.1:6000

  # Terminal C: connect to proxy and type
  nc 127.0.0.1 5001
  ```

## Logging

- Uses `tracing` with environment-based filtering. Default level is `info`.
- Control verbosity with `RUST_LOG`:
  - `RUST_LOG=warn tcp-proxy ...` (only warnings and errors)
  - `RUST_LOG=info tcp-proxy ...` (normal output; default)
  - `RUST_LOG=tcp_proxy=debug tcp-proxy ...` (enable debug for this crate only)
- Connection context: logs emitted while handling a connection are prefixed with a span like `conn{id=..., client=..., remote=...}`.


## Notes

- On Ctrl+C, the proxy stops accepting new connections and waits for existing ones to complete.
- On connect timeout, the client socket is closed and the attempt is logged as an error.
- On session timeout, both sockets are closed and a warning is logged.
- No authentication, authorization, or TLS.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
