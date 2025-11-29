# harmonia-utils-hash

Cryptographic hash utilities for content addressing.

**Contents** (from Nix.rs):
- `Hash` - Generic hash type supporting MD5, SHA1, SHA256, SHA512
- `Algorithm` - Hash algorithm enum with size and digest operations
- `Sha256` / `NarHash` - Specialized hash types for common use cases
- `Context` - Multi-step (Init-Update-Finish) digest calculation
- `HashSink` - Async writer that computes hash of written data
- `fmt` - Hash formatting (Base16, Base32, Base64, SRI)

**Example API**:
```rust
// Hash computation
let hash = Algorithm::SHA256.digest(b"hello, world");

// Multi-step hashing
let mut ctx = Context::new(Algorithm::SHA256);
ctx.update("hello");
ctx.update(", world");
let hash = ctx.finish();

// Hash formatting
let base32 = hash.as_base32().to_string();  // "1b8m03r63zqh..."
let sri = hash.sri().to_string();           // "sha256-ungWv48B..."
```
