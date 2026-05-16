pub(crate) mod to_store_path;
mod types;

pub use types::{
    ContentAddress, ContentAddressMethod, ContentAddressMethodAlgorithm, ParseContentAddressError,
};
/// Compute a content-addressed store path.
pub fn make_store_path_from_ca(
    store_dir: &harmonia_store_path::StoreDir,
    name: harmonia_store_path::StorePathName,
    ca: ContentAddress,
) -> harmonia_store_path::StorePath {
    let path_type = ca.into();
    let fingerprint = to_store_path::Fingerprint {
        name: &name,
        path_type,
    };
    let finger_print_s = store_dir.display(&fingerprint).to_string();
    harmonia_store_path::StorePath::from_hash(
        &harmonia_utils_hash::Sha256::digest(finger_print_s),
        name,
    )
}

#[cfg(test)]
mod unittests {
    use harmonia_utils_hash::fmt::{Any, Bare, Base16};
    use harmonia_utils_hash::{Hash, Sha256};
    use rstest::rstest;

    use super::{ContentAddress, make_store_path_from_ca};
    use harmonia_store_path::{StoreDir, StorePath, StorePathName};

    #[rstest]
    #[case::text(
        ContentAddress::Text("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse::<Bare<Base16<Sha256>>>().unwrap().into()),
        "konsole-18.12.3",
        None,
        "text:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1:/nix/store:konsole-18.12.3",
        "aidi01pgcl6i79fkw737qzx06kjl930m-konsole-18.12.3"
    )]
    #[case::source(
        ContentAddress::NixArchive("sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse::<Any<Hash>>().unwrap().into()),
        "konsole-18.12.3",
        None,
        "source:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1:/nix/store:konsole-18.12.3",
        "1w01xxn8f7s9s4n65ry6rwd7x9awf04s-konsole-18.12.3"
    )]
    #[case::output(
        ContentAddress::NixArchive("sha1:84983e441c3bd26ebaae4aa1f95129e5e54670f1".parse::<Any<Hash>>().unwrap().into()),
        "konsole-18.12.3",
        Some("fixed:out:r:sha1:84983e441c3bd26ebaae4aa1f95129e5e54670f1:"),
        "output:out:sha256:5341f5afdd0fb724c8f7eae0e346de5bb151a00422d47ae683aed85cd78f7120:/nix/store:konsole-18.12.3",
        "ww9d58nz1xsl5ck0vcpc99h23l1y2hln-konsole-18.12.3"
    )]
    #[case::flat_output(
        ContentAddress::Flat("sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse::<Any<Hash>>().unwrap().into()),
        "konsole-18.12.3",
        Some("fixed:out:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1:"),
        "output:out:sha256:e55d6c8c9a08e91f15d5344612c42305702f04f08c487a7aff0b56c4c4add3e7:/nix/store:konsole-18.12.3",
        "jw8chmp9sf8f7pw684cszp6pa2zmn0bx-konsole-18.12.3"
    )]
    // Regression test: real-world fetchurl FOD (libssh2-1.11.1.tar.gz)
    // verified against `nix-store -q --outputs` on the actual derivation.
    #[case::flat_output_libssh2(
        ContentAddress::Flat("sha256:d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7".parse::<Any<Hash>>().unwrap().into()),
        "libssh2-1.11.1.tar.gz",
        Some("fixed:out:sha256:d9ec76cbe34db98eec3539fe2c899d26b0c837cb3eb466a56b0f109cabf658f7:"),
        "output:out:sha256:00ab8c141988e4eedd4695bda86a40373b1f87efe846e29a81a28929a657ee2c:/nix/store:libssh2-1.11.1.tar.gz",
        "j04yfblg6sk5abb4n067xv0x0dfraf73-libssh2-1.11.1.tar.gz"
    )]
    fn test_make_store_path_from_ca(
        #[case] ca: ContentAddress,
        #[case] name: StorePathName,
        #[case] inner_print: Option<&str>,
        #[case] fingerprint: &str,
        #[case] final_path: StorePath,
    ) {
        let expected_hash = harmonia_utils_hash::Sha256::digest(fingerprint);
        let expected_path = StorePath::from_hash(&expected_hash, name.clone());
        let store_dir = StoreDir::default();
        if let Some(print) = inner_print {
            let hash = harmonia_utils_hash::Sha256::digest(print);
            let actual_fingerprint = format!("output:out:sha256:{hash:x}:{store_dir}:{name}");
            assert_eq!(actual_fingerprint, fingerprint);
        }
        let actual_path = make_store_path_from_ca(&store_dir, name, ca);
        assert_eq!(expected_path, actual_path);
        assert_eq!(final_path, actual_path);
    }
}
