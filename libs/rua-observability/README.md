# rua-observability

Observability utilities for rua: structured logging and optional Prometheus metrics.

## Tracing (always on)

```rust
rua_observability::init_tracing(&working_dir);
```

Configures `tracing-subscriber` with daily-rotating file output and 7-day retention.

## Metrics (feature-gated)

Disabled by default. Enable via Cargo feature:

```toml
rua-observability = { path = "../libs/rua-observability", features = ["metrics"] }
```

### Usage

```rust
use std::net::SocketAddr;

let addr: SocketAddr = "127.0.0.1:9090".parse().unwrap();
rua_observability::init_metrics(addr).expect("failed to start metrics exporter");
// curl http://127.0.0.1:9090/metrics
```

### Workspace config (config.json)

```json
{
  "observability": {
    "metrics": {
      "enabled": true,
      "bind_addr": "127.0.0.1:9090"
    }
  }
}
```

When `enabled` is `true` and the `metrics` feature is compiled in, the server starts a Prometheus exporter on `bind_addr` at startup. Default `bind_addr` is `127.0.0.1:9090`.

### rua-server feature

```bash
cargo build -p rua-server --features metrics
```

The server reads `observability.metrics` from `config.json` and starts the exporter automatically.
