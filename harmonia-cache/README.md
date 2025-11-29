# harmonia-cache

Binary cache server for Nix store paths.

## Overview

This crate provides an HTTP server that serves Nix store paths as a binary cache, compatible with the Nix binary cache protocol. It can be used as a drop-in replacement for nix-serve with additional features.

## Features

- **NAR serving**: Serve store paths as NAR archives
- **NARinfo**: Provide path metadata in standard format
- **NAR listings**: Directory listings for store paths (`*.ls`)
- **Build logs**: Serve build logs for derivations
- **File serving**: Direct file access from store paths
- **Prometheus metrics**: Built-in metrics endpoint
- **TLS support**: Optional HTTPS with rustls
- **Unix socket support**: Bind to Unix domain sockets
- **Compression**: Optional zstd response compression

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `/` | Root page with cache info |
| `/{hash}.narinfo` | Path metadata |
| `/{hash}.ls` | Directory listing |
| `/nar/{hash}.nar` | NAR archive |
| `/serve/{hash}/...` | Direct file access |
| `/log/{drv}` | Build log for derivation |
| `/nix-cache-info` | Cache configuration |
| `/health` | Health check |
| `/metrics` | Prometheus metrics |
| `/version` | Server version |

## Configuration

Configuration via `harmonia.toml` or environment variables:

```toml
bind = "0.0.0.0:5000"
workers = 4
sign_key_paths = ["/path/to/secret-key"]

# Optional TLS
tls_cert_path = "/path/to/cert.pem"
tls_key_path = "/path/to/key.pem"
```

## Key Characteristics

- **High performance**: Actix-web with connection pooling to daemon
- **nix-serve compatible**: Drop-in replacement with same URL structure
- **Prometheus integration**: Monitor cache performance
- **Flexible binding**: TCP, TLS, or Unix sockets
