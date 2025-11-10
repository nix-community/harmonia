use std::collections::BTreeMap;


#[cfg(any(test, feature = "test"))]
use proptest::prelude::{Arbitrary, BoxedStrategy};

use crate::ByteString;
use crate::store_path::{StorePath, StorePathSet};

use super::DerivationOutputs;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BasicDerivation {
    pub drv_path: StorePath,
    pub outputs: DerivationOutputs,
    pub input_srcs: StorePathSet,
    pub platform: ByteString,
    pub builder: ByteString,
    pub args: Vec<ByteString>,
    pub env: BTreeMap<ByteString, ByteString>,
}

#[cfg(any(test, feature = "test"))]
pub mod arbitrary {
    use super::*;
    use crate::{
        derivation::derivation_output::arbitrary::arb_derivation_outputs,
        test::arbitrary::arb_byte_string,
    };
    use ::proptest::prelude::*;
    use proptest::sample::SizeRange;

    impl Arbitrary for BasicDerivation {
        type Parameters = ();
        type Strategy = BoxedStrategy<BasicDerivation>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_basic_derivation().boxed()
        }
    }

    prop_compose! {
        pub fn arb_basic_derivation()
        (
            outputs in arb_derivation_outputs(1..15),
            input_srcs in any::<StorePathSet>(),
            platform in arb_byte_string(),
            builder in arb_byte_string(),
            args in proptest::collection::vec(arb_byte_string(), SizeRange::default()),
            env in proptest::collection::btree_map(arb_byte_string(), arb_byte_string(), SizeRange::default()),
            drv_path in any::<StorePath>()
        ) -> BasicDerivation
        {
            BasicDerivation {
                outputs, input_srcs, platform, builder, args, env, drv_path,
            }
        }
    }
}
