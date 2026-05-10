# trusty-analyzer

Sidecar code-analysis daemon for [trusty-search](../trusty-search). Fetches chunk
corpora from the trusty-search daemon, runs static analysis, and serves results via
HTTP (port 7879) and MCP stdio.

## Quick Start

```bash
# trusty-search must be running first
trusty-search daemon

# Run the analyzer sidecar
trusty-analyzer serve --search-url http://127.0.0.1:7878

# Analyze an index
trusty-analyzer analyze <index-id> --top-k 20
```

## Features

- Cyclomatic and cognitive complexity per chunk, file, and index
- Code smell detection with configurable thresholds
- Quality grade aggregation (A–F)
- Git blame temporal decay scoring
- Concept clustering (k-means over embeddings)
- Facts store: `(subject, predicate, object)` knowledge triples
- Full HTTP API + MCP stdio server (tool parity)

## Workspace

```
crates/
  trusty-common/          shared types (also used by trusty-search)
  trusty-analyzer-core/   analysis engines
  trusty-analyzer-service/ axum HTTP daemon
  trusty-analyzer-mcp/    MCP stdio server
src/main.rs               CLI binary
```

## Development

```bash
cargo build
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
```

See [CLAUDE.md](./CLAUDE.md) for full architecture, API reference, and project history.
