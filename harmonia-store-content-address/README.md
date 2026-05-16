# harmonia-store-content-address

Content addressing for the Nix store.

This crate provides the `ContentAddress` type (text, flat, NAR) and the
`make_store_path_from_ca` function that computes a store path from a
content address.

It depends on `harmonia-store-path` (for `StorePath`, `StoreDir`) and
`harmonia-utils-hash` (for hash types).
