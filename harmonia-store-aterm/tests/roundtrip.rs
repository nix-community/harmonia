//! Roundtrip tests for hand-crafted ATerm strings covering specific output
//! variants and edge cases.

use harmonia_store_aterm::{parse_derivation_aterm, print_derivation_aterm};
use harmonia_store_core::store_path::StoreDir;
use rstest::rstest;

#[rstest]
#[case::deferred(
    r#"Derive([("out","","","")],[],[],"x86_64-linux","/bin/sh",[],[("name","test")])"#,
    "test"
)]
#[case::ca_floating(
    r#"Derive([("out","","r:sha256","")],[],[],"x86_64-linux","/bin/sh",[],[("name","test")])"#,
    "test"
)]
#[case::impure(
    r#"Derive([("out","","r:sha256","impure")],[],[],"x86_64-linux","/bin/sh",[],[("name","test")])"#,
    "test",
)]
#[case::escape_sequences(
    r#"Derive([("out","","","")],[],[],"x86_64-linux","/bin/sh",[],[("msg","hello\nworld\t\"test\"\\end"),("name","test")])"#,
    "test",
)]
#[case::input_addressed(
    r#"Derive([("info","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-gnused-4.9-info","",""),("out","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-gnused-4.9","","")],[("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bootstrap-tools.drv",["out"]),("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-stdenv-linux.drv",["out"])],["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-default-builder.sh"],"x86_64-linux","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bootstrap-tools/bin/bash",["-e","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-stdenv.sh"],[("name","gnused-4.9"),("out","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-gnused-4.9"),("system","x86_64-linux")])"#,
    "gnused-4.9",
)]
fn aterm_roundtrips(#[case] input: &str, #[case] name: &str) {
    let store_dir = StoreDir::default();
    let drv = parse_derivation_aterm(&store_dir, input, name.parse().unwrap()).unwrap();
    assert_eq!(print_derivation_aterm(&store_dir, &drv), input);
}

/// CA fixed is special: the parser accepts any path but the printer recomputes
/// the correct path from the content address, so a dummy path won't roundtrip
/// byte-for-byte. Instead we verify the structure survives a round trip.
#[test]
fn roundtrip_ca_fixed() {
    let store_dir = StoreDir::default();
    let input = r#"Derive([("out","/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-linux-6.16.tar.xz","sha256","1a4be2fe6b5246aa4ac8987a8a4af34c42a8dd7d08b46ab48516bcc1befbcd83")],[],[],"builtin","builtin:fetchurl",[],[("name","linux-6.16.tar.xz")])"#;

    let drv =
        parse_derivation_aterm(&store_dir, input, "linux-6.16.tar.xz".parse().unwrap()).unwrap();
    let output = print_derivation_aterm(&store_dir, &drv);

    // Re-parse the printed output and verify structural equality
    let drv2 =
        parse_derivation_aterm(&store_dir, &output, "linux-6.16.tar.xz".parse().unwrap()).unwrap();
    assert_eq!(drv.outputs, drv2.outputs);
    assert_eq!(drv.env, drv2.env);

    // Verify the output has a real computed path and preserves the hash
    assert!(output.contains("/nix/store/"));
    assert!(output.contains("1a4be2fe6b5246aa4ac8987a8a4af34c42a8dd7d08b46ab48516bcc1befbcd83"));
}
