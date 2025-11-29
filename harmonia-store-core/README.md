# harmonia-store-core (Core)

**Purpose**:
Pure store semantics, agnostic to IO / implementation strategy in general.
This is the "business logic" of Nix, pure and simple.
It should be usable with a wide variety of implementation strategies, not forcing any decisions.
It should also be widely usable by other tools which need to engage with Nix (e.g. tools that create dynamic derivations from other build systems' build plans).

**Contents** (from Nix.rs):
- `hash/` - Hash types, algorithms, content addressing
- `store_path/` - Store path parsing, validation, manipulation
- `derivation/` - Derivation (.drv) file format and semantics
- `signature/` - Cryptographic signatures for store paths
- `realisation/` - Store path realisation tracking

**Key Characteristic**: No `async`, no filesystem access, no network
- All operations are pure computations
- Can be tested without IO
- Can be compiled to WASM

**Example API**:
```rust
// Pure computation - no IO
pub fn parse_store_path(path: &str) -> Result<StorePath, ParseError>;
pub fn compute_hash(content: &[u8], hash_type: HashType) -> Hash;
pub fn verify_signature(path: &StorePath, sig: &Signature) -> bool;
```
