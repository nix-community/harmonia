//! SHA-1/256/512 backends: aws-lc on native targets, RustCrypto on wasm.

#[cfg(not(target_family = "wasm"))]
pub(crate) use native::{ShaContext, digest_sha};
#[cfg(target_family = "wasm")]
pub(crate) use portable::{ShaContext, digest_sha};

#[cfg(not(target_family = "wasm"))]
mod native {
    use aws_lc_rs::digest::{self, Digest};

    use crate::{Algorithm, Hash};

    fn aws_lc_algorithm(algorithm: Algorithm) -> &'static digest::Algorithm {
        match algorithm {
            Algorithm::SHA1 => &digest::SHA1_FOR_LEGACY_USE_ONLY,
            Algorithm::SHA256 => &digest::SHA256,
            Algorithm::SHA512 => &digest::SHA512,
            _ => unreachable!("not a SHA algorithm"),
        }
    }

    #[derive(Clone)]
    pub(crate) struct ShaContext(digest::Context);

    impl ShaContext {
        pub(crate) fn new(algorithm: Algorithm) -> Self {
            Self(digest::Context::new(aws_lc_algorithm(algorithm)))
        }

        pub(crate) fn update(&mut self, data: &[u8]) {
            self.0.update(data);
        }

        pub(crate) fn finish(self) -> Digest {
            self.0.finish()
        }
    }

    pub(crate) fn digest_sha(algorithm: Algorithm, data: &[u8]) -> Hash {
        let digest = digest::digest(aws_lc_algorithm(algorithm), data);
        Hash::new(algorithm, digest.as_ref())
    }
}

#[cfg(target_family = "wasm")]
mod portable {
    use sha1::Digest as _;

    use crate::{Algorithm, Hash};

    #[derive(Clone)]
    pub(crate) enum ShaContext {
        Sha1(sha1::Sha1),
        Sha256(sha2::Sha256),
        Sha512(sha2::Sha512),
    }

    impl ShaContext {
        pub(crate) fn new(algorithm: Algorithm) -> Self {
            match algorithm {
                Algorithm::SHA1 => Self::Sha1(sha1::Sha1::new()),
                Algorithm::SHA256 => Self::Sha256(sha2::Sha256::new()),
                Algorithm::SHA512 => Self::Sha512(sha2::Sha512::new()),
                _ => unreachable!("not a SHA algorithm"),
            }
        }

        pub(crate) fn update(&mut self, data: &[u8]) {
            match self {
                Self::Sha1(c) => c.update(data),
                Self::Sha256(c) => c.update(data),
                Self::Sha512(c) => c.update(data),
            }
        }

        pub(crate) fn finish(self) -> Vec<u8> {
            match self {
                Self::Sha1(c) => c.finalize().to_vec(),
                Self::Sha256(c) => c.finalize().to_vec(),
                Self::Sha512(c) => c.finalize().to_vec(),
            }
        }
    }

    pub(crate) fn digest_sha(algorithm: Algorithm, data: &[u8]) -> Hash {
        match algorithm {
            Algorithm::SHA1 => Hash::new(algorithm, sha1::Sha1::digest(data).as_slice()),
            Algorithm::SHA256 => Hash::new(algorithm, sha2::Sha256::digest(data).as_slice()),
            Algorithm::SHA512 => Hash::new(algorithm, sha2::Sha512::digest(data).as_slice()),
            _ => unreachable!("not a SHA algorithm"),
        }
    }
}
