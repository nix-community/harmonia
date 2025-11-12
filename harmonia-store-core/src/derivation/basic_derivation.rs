use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use proptest::prelude::{Arbitrary, BoxedStrategy};

use crate::ByteString;
use crate::store_path::{StorePath, StorePathSet};

use super::DerivationOutputs;

pub struct DerivationInputs {
    pub srcs: StorePathSet,
    pub drvs: BTreeMap<StorePath, BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivationT<Inputs> {
    pub drv_path: StorePath,
    pub outputs: DerivationOutputs,
    pub inputs: Inputs,
    pub platform: ByteString,
    pub builder: ByteString,
    pub args: Vec<ByteString>,
    pub env: BTreeMap<ByteString, ByteString>,
}

pub type BasicDerivation = DerivationT<StorePathSet>;
pub type Derivation = DerivationT<DerivationInputs>;

#[cfg(test)]
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
            inputs in any::<StorePathSet>(),
            platform in arb_byte_string(),
            builder in arb_byte_string(),
            args in proptest::collection::vec(arb_byte_string(), SizeRange::default()),
            env in proptest::collection::btree_map(arb_byte_string(), arb_byte_string(), SizeRange::default()),
            drv_path in any::<StorePath>()
        ) -> BasicDerivation
        {
            DerivationT {
                outputs, inputs, platform, builder, args, env, drv_path,
            }
        }
    }
}
