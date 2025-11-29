# harmonia-daemon (Implementation)

**Purpose**: Daemon server implementation

**Contents** (from Nix.rs):
- `daemon/` - Server logic, socket handling
- Store operations implementation
- Worker threads/connection management

**Key Characteristic**: Ties everything together
- Uses harmonia-store-core for semantics
- Uses harmonia-nar for archive operations
- Uses harmonia-protocol for communication
- Adds IO effects (filesystem, sockets)

**Example API**:
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
