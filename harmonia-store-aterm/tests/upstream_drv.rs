//! Tests that verify ATerm parsing and printing against upstream Nix test data.
//!
//! Each upstream .drv file is parsed with our ATerm parser and compared against
//! the corresponding .json file deserialized into the same [`Derivation`] type.
//! The parsed derivation is then printed back to ATerm and compared with the
//! original .drv file content.

use harmonia_store_aterm::{parse_derivation_aterm, print_derivation_aterm};
use harmonia_store_core::derivation::Derivation;
use harmonia_store_core::store_path::StoreDir;
use harmonia_utils_test::json_upstream::libstore_test_data_path;
use rstest::rstest;

#[rstest]
#[case::simple("derivation/simple-derivation")]
#[case::ca_advanced_attributes("derivation/ca/advanced-attributes")]
#[case::ca_advanced_attributes_defaults("derivation/ca/advanced-attributes-defaults")]
#[case::ca_advanced_attributes_structured_attrs(
    "derivation/ca/advanced-attributes-structured-attrs"
)]
#[case::ca_advanced_attributes_structured_attrs_defaults(
    "derivation/ca/advanced-attributes-structured-attrs-defaults"
)]
#[case::ia_advanced_attributes("derivation/ia/advanced-attributes")]
#[case::ia_advanced_attributes_defaults("derivation/ia/advanced-attributes-defaults")]
#[case::ia_advanced_attributes_structured_attrs(
    "derivation/ia/advanced-attributes-structured-attrs"
)]
#[case::ia_advanced_attributes_structured_attrs_defaults(
    "derivation/ia/advanced-attributes-structured-attrs-defaults"
)]
fn drv_matches_json_and_roundtrips(#[case] base_path: &str) {
    let store_dir = StoreDir::default();

    let drv_path = libstore_test_data_path(&format!("{base_path}.drv"));
    let drv_str = std::fs::read_to_string(&drv_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", drv_path.display()));

    let json_path = libstore_test_data_path(&format!("{base_path}.json"));
    let json_str = std::fs::read_to_string(&json_path)
        .unwrap_or_else(|e| panic!("failed to parse JSON {}: {e}", json_path.display()));

    let from_json: Derivation = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("failed to parse JSON {}: {e}", json_path.display()));

    // Use the name from JSON since ATerm doesn't encode it
    let from_aterm = parse_derivation_aterm(&store_dir, &drv_str, from_json.name.clone())
        .unwrap_or_else(|e| panic!("failed to parse ATerm {}: {e}", drv_path.display()));

    assert_eq!(from_aterm, from_json);

    // Print back to ATerm and verify roundtrip
    let printed = print_derivation_aterm(&store_dir, &from_aterm);
    assert_eq!(printed, drv_str);
}
