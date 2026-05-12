//! Worker protocol wire tests for collection types (`vector` and `set`).
//!
//! These upstream fixtures use heterogeneous tuples, so we use tuple
//! types rather than the homogeneous `Vec<T>` macro.

use std::collections::BTreeSet;

use crate::{no_features, test_upstream_wire_single};

test_upstream_wire_single!(
    vector,
    "vector",
    (Vec<String>, Vec<String>, Vec<String>, Vec<Vec<String>>),
    no_features()
);

test_upstream_wire_single!(
    set,
    "set",
    (
        BTreeSet<String>,
        BTreeSet<String>,
        BTreeSet<String>,
        BTreeSet<BTreeSet<String>>,
    ),
    no_features()
);
