//! BuildResult types with sum type design matching upstream Nix.
//!
//! This module provides BuildResult using a `std::variant<Success, Failure>` pattern
//! that matches the upstream Nix implementation.

use std::collections::BTreeMap;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::daemon_wire::types2::Microseconds;
use crate::de::{Error as _, NixDeserialize as NixDeserializeTrait, NixRead};
use crate::ser::{NixSerialize as NixSerializeTrait, NixWrite};
use crate::types::{DaemonInt, DaemonString, DaemonTime};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::realisation::Realisation;

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
    pub built_outputs: BTreeMap<OutputName, Realisation>,
}

/// Failed build result data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BuildResultFailure {
    pub status: FailureStatus,
    /// Information about the error if the build failed.
    pub error_msg: DaemonString,
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
    pub times_built: DaemonInt,
    /// The start time of the build.
    pub start_time: DaemonTime,
    /// The stop time of the build.
    pub stop_time: DaemonTime,
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

// Wire protocol serialization - maintains compatibility with the flat status enum format

impl NixDeserializeTrait for BuildResult {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        let Some(status_raw) = reader.try_read_value::<u16>().await? else {
            return Ok(None);
        };
        let error_msg: DaemonString = reader.read_value().await?;
        let times_built: DaemonInt = reader.read_value().await?;
        let is_non_deterministic: bool = reader.read_value().await?;
        let start_time: DaemonTime = reader.read_value().await?;
        let stop_time: DaemonTime = reader.read_value().await?;
        let cpu_user: Option<Microseconds> = reader.read_value().await?;
        let cpu_system: Option<Microseconds> = reader.read_value().await?;
        let built_outputs: BTreeMap<OutputName, Realisation> = reader.read_value().await?;

        let inner = if let Ok(status) = SuccessStatus::try_from(status_raw) {
            BuildResultInner::Success(BuildResultSuccess {
                status,
                built_outputs,
            })
        } else if let Ok(status) = FailureStatus::try_from(status_raw) {
            BuildResultInner::Failure(BuildResultFailure {
                status,
                error_msg,
                is_non_deterministic,
            })
        } else {
            return Err(R::Error::invalid_data(format!(
                "invalid build status: {}",
                status_raw
            )));
        };

        Ok(Some(BuildResult {
            inner,
            times_built,
            start_time,
            stop_time,
            cpu_user,
            cpu_system,
        }))
    }
}

impl NixSerializeTrait for BuildResult {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let (status_raw, error_msg, is_non_deterministic, built_outputs): (
            u16,
            &DaemonString,
            bool,
            &BTreeMap<OutputName, Realisation>,
        ) = match &self.inner {
            BuildResultInner::Success(s) => {
                static EMPTY_STRING: DaemonString = DaemonString::new();
                (s.status.into(), &EMPTY_STRING, false, &s.built_outputs)
            }
            BuildResultInner::Failure(f) => {
                static EMPTY_MAP: BTreeMap<OutputName, Realisation> = BTreeMap::new();
                (
                    f.status.into(),
                    &f.error_msg,
                    f.is_non_deterministic,
                    &EMPTY_MAP,
                )
            }
        };

        writer.write_value(&status_raw).await?;
        writer.write_value(error_msg).await?;
        writer.write_value(&self.times_built).await?;
        writer.write_value(&is_non_deterministic).await?;
        writer.write_value(&self.start_time).await?;
        writer.write_value(&self.stop_time).await?;
        writer.write_value(&self.cpu_user).await?;
        writer.write_value(&self.cpu_system).await?;
        writer.write_value(built_outputs).await?;

        Ok(())
    }
}

// JSON serialization for upstream Nix compatibility

/// JSON helper for BuildResultFailure (handles DaemonString <-> String conversion)
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
            error_msg: json.error_msg.into(),
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
