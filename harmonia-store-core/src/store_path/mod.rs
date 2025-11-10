use std::collections::BTreeSet;

mod content_address;
mod create;
mod path;
mod store_dir;

pub use content_address::{ContentAddress, ContentAddressMethod, ContentAddressMethodAlgorithm};
pub(crate) use path::into_name;
pub use path::{
    ParseStorePathError, StorePath, StorePathError, StorePathHash, StorePathName,
    StorePathNameError,
};
pub use store_dir::{FromStoreDirStr, StoreDir, StoreDirDisplay};

pub type StorePathSet = BTreeSet<StorePath>;

#[cfg(any(test, feature = "test"))]
pub mod proptest {
    pub use super::path::proptest::*;
    pub use super::store_dir::proptest::*;
}
