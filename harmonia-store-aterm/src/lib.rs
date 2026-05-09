mod error;
mod parser;
mod printer;

pub use error::ParseError;
pub use printer::print_derivation_aterm;

use harmonia_store_core::derivation::Derivation;
use harmonia_store_core::store_path::{StoreDir, StorePathName};

/// Parse a Nix derivation in [ATerm format](https://nix.dev/manual/nix/latest/protocols/derivation-aterm.html)
/// into a [`Derivation`].
///
/// The `name` is the derivation name, typically extracted from the `.drv` file's
/// store path (e.g. `"hello-2.12.2"` from `/nix/store/...-hello-2.12.2.drv`).
///
/// The `store_dir` is needed to convert absolute store paths in the ATerm
/// (e.g. `/nix/store/hash-name`) into [`StorePath`](harmonia_store_core::store_path::StorePath) values.
pub fn parse_derivation_aterm(
    store_dir: &StoreDir,
    input: &str,
    name: StorePathName,
) -> Result<Derivation, ParseError> {
    parser::Parser::new(input, store_dir).parse_derivation(name)
}
