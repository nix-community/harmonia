//! SHA-1/256/512 backends: aws-lc on native targets, RustCrypto on wasm.

#[cfg(not(target_family = "wasm"))]
pub(crate) use native::ShaContext;
#[cfg(target_family = "wasm")]
pub(crate) use portable::ShaContext;

#[cfg(not(target_family = "wasm"))]
mod native {
    use aws_lc_rs::digest::{self, Digest};

    use crate::Algorithm;

    #[derive(Clone)]
    pub(crate) struct ShaContext(digest::Context);

    impl ShaContext {
        pub(crate) fn new(algorithm: Algorithm) -> Self {
            let algo = match algorithm {
                Algorithm::SHA1 => &digest::SHA1_FOR_LEGACY_USE_ONLY,
                Algorithm::SHA256 => &digest::SHA256,
                Algorithm::SHA512 => &digest::SHA512,
                _ => unreachable!("not a SHA algorithm"),
            };
            Self(digest::Context::new(algo))
        }

        pub(crate) fn update(&mut self, data: &[u8]) {
            self.0.update(data);
        }

        pub(crate) fn finish(self) -> Digest {
            self.0.finish()
        }
    }
}

#[cfg(target_family = "wasm")]
mod portable {
    use sha1::Digest as _;

    use crate::Algorithm;

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
}
