# harmonia-protocol (Protocol)

**Purpose**: Daemon wire protocol definition

**Contents** (from Nix.rs):
- `wire/` - Protocol message types
- Serialization/deserialization for protocol
- Derive macros for protocol messages (from nixrs-derive)

**Key Characteristic**: Protocol-focused
- Defines the contract between client and daemon
- Version negotiation
- Operation encoding/decoding

**Example API**:
```rust
#[derive(NixProtocol)]
pub enum Operation {
    QueryValidPaths { paths: Vec<StorePath> },
    QueryPathInfo { path: StorePath },
    NarFromPath { path: StorePath },
    // ...
}

pub trait ProtocolCodec {
    async fn read_operation<R: AsyncRead>(&mut self, reader: R) -> Result<Operation>;
    async fn write_operation<W: AsyncWrite>(&mut self, writer: W, op: &Operation) -> Result<()>;
}
```
