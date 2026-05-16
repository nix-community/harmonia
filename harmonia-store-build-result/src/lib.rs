// SPDX-FileCopyrightText: 2025 Obsidian Systems
// SPDX-License-Identifier: MIT

//! Pure `BuildResult` types and JSON serialization for Nix build results.
//!
//! Quarantined from `harmonia-protocol` so the types can be used without
//! pulling in the full wire protocol stack.  Protocol-specific wire format
//! impls are added in the protocol layer.

use std::collections::BTreeMap;
use std::time::Duration;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use harmonia_store_derivation::derived_path::OutputName;
use harmonia_store_derivation::realisation::UnkeyedRealisation;

/// A duration measured in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Microseconds(pub i64);

impl From<i64> for Microseconds {
    fn from(value: i64) -> Self {
        Microseconds(value)
    }
}

impl From<Microseconds> for i64 {
    fn from(value: Microseconds) -> Self {
        value.0
    }
}

impl From<Microseconds> for Duration {
    fn from(value: Microseconds) -> Self {
        Duration::from_micros(value.0.unsigned_abs())
    }
}

impl TryFrom<Duration> for Microseconds {
    type Error = std::num::TryFromIntError;
    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Ok(Microseconds(value.as_micros().try_into()?))
    }
}

/// Success status values for BuildResult.
///
/// Must be disjoint with `FailureStatus`.
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
    Serialize,
    Deserialize,
)]
#[repr(u16)]
pub enum SuccessStatus {
    Built = 0,
    Substituted = 1,
    AlreadyValid = 2,
    ResolvesToAlreadyValid = 13,
}

/// Failure status values for BuildResult.
///
/// Must be disjoint with `SuccessStatus`.
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
    Serialize,
    Deserialize,
)]
#[repr(u16)]
pub enum FailureStatus {
    PermanentFailure = 3,
    InputRejected = 4,
    OutputRejected = 5,
    /// possibly transient
    TransientFailure = 6,
    /// no longer used
    CachedFailure = 7,
    TimedOut = 8,
    MiscFailure = 9,
    DependencyFailed = 10,
    LogLimitExceeded = 11,
    NotDeterministic = 12,
    NoSubstituters = 14,
}

/// Successful build result data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResultSuccess {
    pub status: SuccessStatus,
    /// For derivations, a mapping from output names to realisations.
    #[serde(default)]
    pub built_outputs: BTreeMap<OutputName, UnkeyedRealisation>,
}

/// Failed build result data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuildResultFailure {
    pub status: FailureStatus,
    /// Information about the error if the build failed.
    pub error_msg: Vec<u8>,
    /// If timesBuilt > 1, whether some builds did not produce the same result.
    pub is_non_deterministic: bool,
}

/// The inner result of a build - either success or failure.
///
/// Uses the `success` field as a discriminator for JSON serialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BuildResultInner {
    Success(BuildResultSuccess),
    Failure(BuildResultFailure),
}

/// Result of a build operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    #[serde(flatten)]
    pub inner: BuildResultInner,
    /// How many times this build was performed.
    pub times_built: u32,
    /// The start time of the build.
    pub start_time: i64,
    /// The stop time of the build.
    pub stop_time: i64,
    /// User CPU time the build took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_user: Option<Microseconds>,
    /// System CPU time the build took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_system: Option<Microseconds>,
}

impl BuildResult {
    /// Returns the success data if this is a successful build.
    pub fn success(&self) -> Option<&BuildResultSuccess> {
        match &self.inner {
            BuildResultInner::Success(s) => Some(s),
            BuildResultInner::Failure(_) => None,
        }
    }

    /// Returns the failure data if this is a failed build.
    pub fn failure(&self) -> Option<&BuildResultFailure> {
        match &self.inner {
            BuildResultInner::Success(_) => None,
            BuildResultInner::Failure(f) => Some(f),
        }
    }
}

// JSON serialization for upstream Nix compatibility

/// JSON helper for BuildResultFailure (handles Vec<u8> <-> String conversion)
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildResultFailureJson {
    status: FailureStatus,
    #[serde(default)]
    error_msg: String,
    #[serde(default)]
    is_non_deterministic: bool,
}

impl Serialize for BuildResultFailure {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BuildResultFailureJson {
            status: self.status,
            error_msg: String::from_utf8_lossy(&self.error_msg).into_owned(),
            is_non_deterministic: self.is_non_deterministic,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BuildResultFailure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let json = BuildResultFailureJson::deserialize(deserializer)?;
        Ok(BuildResultFailure {
            status: json.status,
            error_msg: json.error_msg.into_bytes(),
            is_non_deterministic: json.is_non_deterministic,
        })
    }
}

impl Serialize for BuildResultInner {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::Error;

        let (success, value) = match self {
            BuildResultInner::Success(s) => {
                (true, serde_json::to_value(s).map_err(S::Error::custom)?)
            }
            BuildResultInner::Failure(f) => {
                (false, serde_json::to_value(f).map_err(S::Error::custom)?)
            }
        };

        let Value::Object(mut map) = value else {
            return Err(S::Error::custom("expected object"));
        };
        map.insert("success".into(), Value::Bool(success));
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BuildResultInner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        let mut map = Map::<String, Value>::deserialize(deserializer)?;

        let success = map
            .remove("success")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| D::Error::missing_field("success"))?;

        let value = Value::Object(map);

        if success {
            serde_json::from_value(value)
                .map(BuildResultInner::Success)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(BuildResultInner::Failure)
                .map_err(D::Error::custom)
        }
    }
}

#[cfg(any(test, feature = "test"))]
mod arbitrary {
    use super::*;
    use proptest::prelude::*;

    impl Arbitrary for Microseconds {
        type Parameters = ();
        type Strategy = BoxedStrategy<Microseconds>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            (0i64..i64::MAX).prop_map(Microseconds).boxed()
        }
    }

    impl Arbitrary for SuccessStatus {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;
        fn arbitrary_with(_: ()) -> Self::Strategy {
            use SuccessStatus::*;
            prop_oneof![
                Just(Built),
                Just(Substituted),
                Just(AlreadyValid),
                Just(ResolvesToAlreadyValid),
            ]
            .boxed()
        }
    }

    impl Arbitrary for FailureStatus {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;
        fn arbitrary_with(_: ()) -> Self::Strategy {
            use FailureStatus::*;
            prop_oneof![
                Just(PermanentFailure),
                Just(InputRejected),
                Just(OutputRejected),
                Just(TransientFailure),
                Just(CachedFailure),
                Just(TimedOut),
                Just(MiscFailure),
                Just(DependencyFailed),
                Just(LogLimitExceeded),
                Just(NotDeterministic),
                Just(NoSubstituters),
            ]
            .boxed()
        }
    }

    impl Arbitrary for BuildResult {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;
        fn arbitrary_with(_: ()) -> Self::Strategy {
            let success = (
                any::<SuccessStatus>(),
                proptest::collection::btree_map(
                    any::<OutputName>(),
                    any::<UnkeyedRealisation>(),
                    0..4,
                ),
            )
                .prop_map(|(status, built_outputs)| {
                    BuildResultInner::Success(BuildResultSuccess {
                        status,
                        built_outputs,
                    })
                });
            let failure = (any::<FailureStatus>(), any::<Vec<u8>>(), any::<bool>()).prop_map(
                |(status, error_msg, is_non_deterministic)| {
                    BuildResultInner::Failure(BuildResultFailure {
                        status,
                        error_msg,
                        is_non_deterministic,
                    })
                },
            );
            (
                prop_oneof![success, failure],
                any::<u32>(),
                any::<i64>(),
                any::<i64>(),
                any::<Option<Microseconds>>(),
                any::<Option<Microseconds>>(),
            )
                .prop_map(
                    |(inner, times_built, start_time, stop_time, cpu_user, cpu_system)| {
                        BuildResult {
                            inner,
                            times_built,
                            start_time,
                            stop_time,
                            cpu_user,
                            cpu_system,
                        }
                    },
                )
                .boxed()
        }
    }
}
