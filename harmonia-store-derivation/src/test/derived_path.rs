use crate::derived_path::DerivedPath;
use harmonia_store_path::StoreDir;

pub fn parse_path(s: &str) -> DerivedPath {
    StoreDir::default().parse(s).unwrap()
}
