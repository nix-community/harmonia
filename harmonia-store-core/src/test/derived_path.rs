use crate::derived_path::DerivedPath;
use crate::store_path::StoreDir;

pub fn parse_path(s: &str) -> DerivedPath {
    StoreDir::default().parse(s).unwrap()
}
