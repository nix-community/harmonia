# harmonia-utils-base-encoding

Base encoding/decoding utilities for the various encodings Nix uses.

**Contents**:
- `base32` - Nix base32 encoding (special 32-character alphabet, LSB first, reversed)
- `Base` - Enum for selecting encoding format (Hex, NixBase32, Base64)
- `decode_for_base` / `encode_for_base` - Get encode/decode functions for a given base
- `base64_len` - Calculate base64 encoded length

**Example API**:
```rust
// Nix base32 encoding
pub mod base32 {
    pub fn encode_string(input: &[u8]) -> String;
    pub fn decode_mut(input: &[u8], output: &mut [u8]) -> Result<usize, DecodePartial>;
}

// Base encoding selection
pub enum Base { Hex, NixBase32, Base64 }
pub fn decode_for_base(base: Base) -> impl Fn(&[u8], &mut [u8]) -> Result<usize, DecodePartial>;
```
