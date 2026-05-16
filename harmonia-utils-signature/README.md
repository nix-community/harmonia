# harmonia-utils-signature

Cryptographic signing of Nix store paths using Ed25519 (NAR signatures).

This crate provides the `Signature`, `SecretKey`, and `PublicKey` types used
throughout the Harmonia workspace for signing and verifying store path
fingerprints. It has no dependency on `harmonia-store-core` — it is a leaf
crate depending only on cryptographic primitives and encoding utilities.
