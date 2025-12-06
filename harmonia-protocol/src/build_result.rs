//! BuildResult types for the daemon wire protocol.

use std::collections::BTreeMap;

use num_enum::{IntoPrimitive, TryFromPrimitive};
#[cfg(test)]
use test_strategy::Arbitrary;

use crate::daemon_wire::types2::Microseconds;
use crate::types::{DaemonInt, DaemonString, DaemonTime};
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use harmonia_store_core::derived_path::DerivedPath;
use harmonia_store_core::realisation::{DrvOutput, Realisation};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    TryFromPrimitive,
    IntoPrimitive,
    NixDeserialize,
    NixSerialize,
)]
#[nix(try_from = "u16", into = "u16")]
#[repr(u16)]
pub enum BuildStatus {
    Built = 0,
    Substituted = 1,
    AlreadyValid = 2,
    PermanentFailure = 3,
    InputRejected = 4,
    OutputRejected = 5,
    TransientFailure = 6,
    CachedFailure = 7,
    TimedOut = 8,
    MiscFailure = 9,
    DependencyFailed = 10,
    LogLimitExceeded = 11,
    NotDeterministic = 12,
    ResolvesToAlreadyValid = 13,
    NoSubstituters = 14,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
pub struct BuildResult {
    pub status: BuildStatus,
    pub error_msg: DaemonString,
    pub times_built: DaemonInt,
    pub is_non_deterministic: bool,
    pub start_time: DaemonTime,
    pub stop_time: DaemonTime,
    pub cpu_user: Option<Microseconds>,
    pub cpu_system: Option<Microseconds>,
    pub built_outputs: BTreeMap<DrvOutput, Realisation>,
}

pub type KeyedBuildResults = Vec<KeyedBuildResult>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct KeyedBuildResult {
    pub path: DerivedPath,
    pub result: BuildResult,
}

#[cfg(test)]
pub mod arbitrary {
    use super::*;
    use ::proptest::prelude::*;
    use harmonia_store_core::realisation::arbitrary::arb_drv_outputs;
    use harmonia_store_core::test::arbitrary::arb_byte_string;

    impl Arbitrary for BuildStatus {
        type Parameters = ();
        type Strategy = BoxedStrategy<BuildStatus>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            use BuildStatus::*;
            prop_oneof![
                50 => Just(Built),
                5 => Just(Substituted),
                5 => Just(AlreadyValid),
                5 => Just(PermanentFailure),
                5 => Just(InputRejected),
                5 => Just(OutputRejected),
                5 => Just(TransientFailure), // possibly transient
                5 => Just(TimedOut),
                5 => Just(MiscFailure),
                5 => Just(DependencyFailed),
                5 => Just(LogLimitExceeded),
                5 => Just(NotDeterministic)
            ]
            .boxed()
        }
    }

    impl Arbitrary for BuildResult {
        type Parameters = ();
        type Strategy = BoxedStrategy<BuildResult>;
        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_build_result().boxed()
        }
    }

    prop_compose! {
        pub fn arb_build_result()
        (
            status in any::<BuildStatus>(),
            error_msg in arb_byte_string(),
            times_built in 0u32..50u32,
            is_non_deterministic in ::proptest::bool::ANY,
            start_time in ::proptest::num::i64::ANY,
            duration_secs in 0i64..604_800i64,
            cpu_user in any::<Option<Microseconds>>(),
            cpu_system in any::<Option<Microseconds>>(),
            built_outputs in arb_drv_outputs(0..5),
        ) -> BuildResult
        {
            let stop_time = start_time + duration_secs;
            BuildResult {
                status, error_msg, times_built, is_non_deterministic,
                start_time, stop_time, cpu_user, cpu_system, built_outputs,
            }
        }
    }
}
