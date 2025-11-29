# harmonia-daemon (Implementation)

**Purpose**: Daemon server implementation

## Overview

This crate provides the server-side implementation of the Nix daemon protocol, handling client connections and store operations.

## Contents

**Contents** (from Nix.rs):
- `daemon/` - Server logic, socket handling
- Store operations implementation
- Worker threads/connection management

## Key Characteristics

Ties everything together:
- Uses harmonia-store-core for semantics
- Uses harmonia-nar for archive operations
- Uses harmonia-protocol for communication
- Adds IO effects (filesystem, sockets)

## Example

```rust
pub struct Daemon {
    store: Store,
    config: DaemonConfig,
}

impl Daemon {
    pub async fn serve(&self, listener: UnixListener) -> Result<()> {
        // Accept connections, handle protocol operations
    }
}
```
