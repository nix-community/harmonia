# harmonia-store-remote (Implementation)

**Purpose**: Daemon client library with connection pooling

## Overview

This crate provides a client implementation for connecting to the Nix daemon and performing store operations over the daemon protocol. It is the Remote Store Layer in Harmonia's store architecture.

## Contents: New implementation for Harmonia

- Protocol client using harmonia-protocol types
- Connection pool with queue management
- Retry logic and error handling
- Metrics and observability hooks

## Key Characteristics

Reusable client library:
- Built-in connection pooling (no separate pool crate needed)
- Typed errors
- Async-first API

## Example

```rust
pub struct Client {
    pool: ConnectionPool,
    config: ClientConfig,
}

impl Client {
    pub async fn query_path_info(&self, path: &StorePath) -> Result<PathInfo>;
    pub async fn nar_from_path(&self, path: &StorePath) -> Result<impl AsyncRead>;

    // Pool management
    pub fn pool_metrics(&self) -> PoolMetrics;
}
```
