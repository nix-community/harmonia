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
use serde::de;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResultFailure {
    pub status: FailureStatus,
    /// Information about the error if the build failed.
    #[serde(default, with = "lossy_utf8")]
    pub error_msg: Vec<u8>,
    /// If timesBuilt > 1, whether some builds did not produce the same result.
    #[serde(default)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuildResult {
    pub inner: BuildResultInner,
    /// How many times this build was performed.
    pub times_built: u32,
    /// The start time of the build.
    pub start_time: i64,
    /// The stop time of the build.
    pub stop_time: i64,
    /// User CPU time the build took.
    pub cpu_user: Option<Microseconds>,
    /// System CPU time the build took.
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

mod lossy_utf8 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&String::from_utf8_lossy(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        String::deserialize(deserializer).map(String::into_bytes)
    }
}

impl BuildResultInner {
    const fn serialized_field_count(&self) -> usize {
        match self {
            BuildResultInner::Success(_) => 3,
            BuildResultInner::Failure(_) => 4,
        }
    }

    fn serialize_entries<S: SerializeMap>(&self, map: &mut S) -> Result<(), S::Error> {
        match self {
            BuildResultInner::Success(ok) => {
                map.serialize_entry("success", &true)?;
                map.serialize_entry("status", &ok.status)?;
                map.serialize_entry("builtOutputs", &ok.built_outputs)?;
            }
            BuildResultInner::Failure(fail) => {
                map.serialize_entry("success", &false)?;
                map.serialize_entry("status", &fail.status)?;
                map.serialize_entry("errorMsg", &String::from_utf8_lossy(&fail.error_msg))?;
                map.serialize_entry("isNonDeterministic", &fail.is_non_deterministic)?;
            }
        }
        Ok(())
    }
}

impl Serialize for BuildResultInner {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.serialized_field_count()))?;
        self.serialize_entries(&mut map)?;
        map.end()
    }
}

impl Serialize for BuildResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let len = self.inner.serialized_field_count()
            + 3
            + usize::from(self.cpu_user.is_some())
            + usize::from(self.cpu_system.is_some());
        let mut map = serializer.serialize_map(Some(len))?;
        self.inner.serialize_entries(&mut map)?;
        map.serialize_entry("timesBuilt", &self.times_built)?;
        map.serialize_entry("startTime", &self.start_time)?;
        map.serialize_entry("stopTime", &self.stop_time)?;
        if let Some(cpu_user) = &self.cpu_user {
            map.serialize_entry("cpuUser", cpu_user)?;
        }
        if let Some(cpu_system) = &self.cpu_system {
            map.serialize_entry("cpuSystem", cpu_system)?;
        }
        map.end()
    }
}

/// Disjoint union of `SuccessStatus` and `FailureStatus` so `status` can be
/// deserialized in a single pass regardless of whether it precedes `success`.
#[derive(Deserialize)]
enum AnyStatus {
    Built,
    Substituted,
    AlreadyValid,
    ResolvesToAlreadyValid,
    PermanentFailure,
    InputRejected,
    OutputRejected,
    TransientFailure,
    CachedFailure,
    TimedOut,
    MiscFailure,
    DependencyFailed,
    LogLimitExceeded,
    NotDeterministic,
    NoSubstituters,
}

impl AnyStatus {
    const fn into_success(self) -> Option<SuccessStatus> {
        match self {
            Self::Built => Some(SuccessStatus::Built),
            Self::Substituted => Some(SuccessStatus::Substituted),
            Self::AlreadyValid => Some(SuccessStatus::AlreadyValid),
            Self::ResolvesToAlreadyValid => Some(SuccessStatus::ResolvesToAlreadyValid),
            _ => None,
        }
    }

    const fn into_failure(self) -> Option<FailureStatus> {
        match self {
            Self::PermanentFailure => Some(FailureStatus::PermanentFailure),
            Self::InputRejected => Some(FailureStatus::InputRejected),
            Self::OutputRejected => Some(FailureStatus::OutputRejected),
            Self::TransientFailure => Some(FailureStatus::TransientFailure),
            Self::CachedFailure => Some(FailureStatus::CachedFailure),
            Self::TimedOut => Some(FailureStatus::TimedOut),
            Self::MiscFailure => Some(FailureStatus::MiscFailure),
            Self::DependencyFailed => Some(FailureStatus::DependencyFailed),
            Self::LogLimitExceeded => Some(FailureStatus::LogLimitExceeded),
            Self::NotDeterministic => Some(FailureStatus::NotDeterministic),
            Self::NoSubstituters => Some(FailureStatus::NoSubstituters),
            _ => None,
        }
    }
}

// Compile-time guard: adding a variant to SuccessStatus or FailureStatus fails here.
const _: fn(SuccessStatus) -> AnyStatus = |s| match s {
    SuccessStatus::Built => AnyStatus::Built,
    SuccessStatus::Substituted => AnyStatus::Substituted,
    SuccessStatus::AlreadyValid => AnyStatus::AlreadyValid,
    SuccessStatus::ResolvesToAlreadyValid => AnyStatus::ResolvesToAlreadyValid,
};
const _: fn(FailureStatus) -> AnyStatus = |f| match f {
    FailureStatus::PermanentFailure => AnyStatus::PermanentFailure,
    FailureStatus::InputRejected => AnyStatus::InputRejected,
    FailureStatus::OutputRejected => AnyStatus::OutputRejected,
    FailureStatus::TransientFailure => AnyStatus::TransientFailure,
    FailureStatus::CachedFailure => AnyStatus::CachedFailure,
    FailureStatus::TimedOut => AnyStatus::TimedOut,
    FailureStatus::MiscFailure => AnyStatus::MiscFailure,
    FailureStatus::DependencyFailed => AnyStatus::DependencyFailed,
    FailureStatus::LogLimitExceeded => AnyStatus::LogLimitExceeded,
    FailureStatus::NotDeterministic => AnyStatus::NotDeterministic,
    FailureStatus::NoSubstituters => AnyStatus::NoSubstituters,
};

fn finish_inner<E: de::Error>(
    success: bool,
    status: AnyStatus,
    built_outputs: Option<BTreeMap<OutputName, UnkeyedRealisation>>,
    error_msg: Option<String>,
    is_non_deterministic: Option<bool>,
) -> Result<BuildResultInner, E> {
    if success {
        let status = status
            .into_success()
            .ok_or_else(|| E::custom("expected SuccessStatus when success is true"))?;
        Ok(BuildResultInner::Success(BuildResultSuccess {
            status,
            built_outputs: built_outputs.unwrap_or_default(),
        }))
    } else {
        let status = status
            .into_failure()
            .ok_or_else(|| E::custom("expected FailureStatus when success is false"))?;
        Ok(BuildResultInner::Failure(BuildResultFailure {
            status,
            error_msg: error_msg.unwrap_or_default().into_bytes(),
            is_non_deterministic: is_non_deterministic.unwrap_or(false),
        }))
    }
}

#[derive(Deserialize)]
#[serde(rename = "BuildResultInner", rename_all = "camelCase")]
struct BuildResultInnerDe {
    success: bool,
    status: AnyStatus,
    #[serde(default)]
    built_outputs: Option<BTreeMap<OutputName, UnkeyedRealisation>>,
    #[serde(default)]
    error_msg: Option<String>,
    #[serde(default)]
    is_non_deterministic: Option<bool>,
}

impl<'de> Deserialize<'de> for BuildResultInner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BuildResultInnerDe::deserialize(deserializer)?;
        finish_inner(
            raw.success,
            raw.status,
            raw.built_outputs,
            raw.error_msg,
            raw.is_non_deterministic,
        )
    }
}

#[derive(Deserialize)]
#[serde(rename = "BuildResult", rename_all = "camelCase")]
struct BuildResultDe {
    success: bool,
    status: AnyStatus,
    #[serde(default)]
    built_outputs: Option<BTreeMap<OutputName, UnkeyedRealisation>>,
    #[serde(default)]
    error_msg: Option<String>,
    #[serde(default)]
    is_non_deterministic: Option<bool>,
    times_built: u32,
    start_time: i64,
    stop_time: i64,
    #[serde(default)]
    cpu_user: Option<Microseconds>,
    #[serde(default)]
    cpu_system: Option<Microseconds>,
}

impl<'de> Deserialize<'de> for BuildResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BuildResultDe::deserialize(deserializer)?;
        let inner = finish_inner(
            raw.success,
            raw.status,
            raw.built_outputs,
            raw.error_msg,
            raw.is_non_deterministic,
        )?;
        Ok(BuildResult {
            inner,
            times_built: raw.times_built,
            start_time: raw.start_time,
            stop_time: raw.stop_time,
            cpu_user: raw.cpu_user,
            cpu_system: raw.cpu_system,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn roundtrip_all_status_variants() {
        for status in [
            SuccessStatus::Built,
            SuccessStatus::Substituted,
            SuccessStatus::AlreadyValid,
            SuccessStatus::ResolvesToAlreadyValid,
        ] {
            let inner = BuildResultInner::Success(BuildResultSuccess {
                status,
                built_outputs: BTreeMap::new(),
            });
            let json = serde_json::to_string(&inner).unwrap();
            let parsed: BuildResultInner = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, inner);
        }

        for status in [
            FailureStatus::PermanentFailure,
            FailureStatus::InputRejected,
            FailureStatus::OutputRejected,
            FailureStatus::TransientFailure,
            FailureStatus::CachedFailure,
            FailureStatus::TimedOut,
            FailureStatus::MiscFailure,
            FailureStatus::DependencyFailed,
            FailureStatus::LogLimitExceeded,
            FailureStatus::NotDeterministic,
            FailureStatus::NoSubstituters,
        ] {
            let failure = BuildResultFailure {
                status,
                error_msg: b"failed".to_vec(),
                is_non_deterministic: status == FailureStatus::NotDeterministic,
            };
            let fail_json = serde_json::to_string(&failure).unwrap();
            assert_eq!(
                serde_json::from_str::<BuildResultFailure>(&fail_json).unwrap(),
                failure
            );

            let inner = BuildResultInner::Failure(failure);
            let json = serde_json::to_string(&inner).unwrap();
            assert_eq!(
                serde_json::from_str::<BuildResultInner>(&json).unwrap(),
                inner
            );
        }
    }

    #[test]
    fn failure_non_utf8_lossy_and_field_ordering() {
        let fail = BuildResultFailure {
            status: FailureStatus::MiscFailure,
            error_msg: vec![b'a', 0xff, b'b'],
            is_non_deterministic: true,
        };
        let json = serde_json::to_string(&fail).unwrap();
        let parsed: BuildResultFailure = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.error_msg, "a�b".as_bytes());
        assert!(parsed.is_non_deterministic);

        let raw = r#"{"status":"Built","success":true,"timesBuilt":1,"startTime":10,"stopTime":20,"cpuUser":null,"cpuSystem":null,"extra":123,"errorMsg":null}"#;
        let res: BuildResult = serde_json::from_str(raw).unwrap();
        assert_eq!(res.success().unwrap().status, SuccessStatus::Built);
        assert!(res.cpu_user.is_none());
        assert!(res.cpu_system.is_none());
    }

    #[test]
    fn rejects_invalid_payloads() {
        assert!(
            serde_json::from_str::<BuildResultInner>(
                r#"{"success":true,"status":"PermanentFailure"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<BuildResultInner>(r#"{"success":false,"status":"Built"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<BuildResultInner>(r#"{"status":"Built"}"#).is_err());
        assert!(
            serde_json::from_str::<BuildResult>(
                r#"{"success":true,"status":"Built","startTime":0,"stopTime":0}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<BuildResult>(
                r#"{"success":true,"status":"Built","timesBuilt":1,"timesBuilt":2,"startTime":0,"stopTime":0}"#
            )
            .is_err()
        );
    }

    proptest! {
        #[test]
        fn proptest_build_result_json_roundtrip(res in any::<BuildResult>()) {
            let mut expected = res;
            if let BuildResultInner::Failure(ref mut f) = expected.inner {
                f.error_msg = String::from_utf8_lossy(&f.error_msg).into_owned().into_bytes();
            }
            let json = serde_json::to_string(&expected).unwrap();
            let parsed: BuildResult = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(parsed, expected);
        }
    }
}
