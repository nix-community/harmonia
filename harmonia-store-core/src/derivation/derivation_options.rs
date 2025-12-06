//! Derivation build-time options.
//!
//! This module implements the `DerivationOptions` type which represents
//! special build-time options for derivations. These options control
//! things like sandboxing, reference checking, and build locality.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::derived_path::{OutputName, SingleDerivedPath};
use crate::drv_ref::DrvRef;
use crate::store_path::StorePath;

/// Constraints on what a specific output can reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    bound(
        serialize = "Input: Serialize + Ord",
        deserialize = "Input: Deserialize<'de> + Ord"
    )
)]
pub struct OutputCheckSpec<Input: Ord> {
    /// Whether references from this output to itself should be ignored when checking references.
    #[serde(default)]
    pub ignore_self_refs: bool,

    /// Maximum allowed size of this output in bytes, or null for no limit.
    #[serde(default)]
    pub max_size: Option<u64>,

    /// Maximum allowed size of this output's closure in bytes, or null for no limit.
    #[serde(default)]
    pub max_closure_size: Option<u64>,

    /// If set, the output can only reference paths in this list.
    /// If null, no restrictions apply.
    #[serde(default)]
    pub allowed_references: Option<BTreeSet<DrvRef<Input>>>,

    /// If set, the output's closure can only contain paths in this list.
    /// If null, no restrictions apply.
    #[serde(default)]
    pub allowed_requisites: Option<BTreeSet<DrvRef<Input>>>,

    /// The output must not reference any paths in this list.
    #[serde(default)]
    pub disallowed_references: BTreeSet<DrvRef<Input>>,

    /// The output's closure must not contain any paths in this list.
    #[serde(default)]
    pub disallowed_requisites: BTreeSet<DrvRef<Input>>,
}

impl<Input: Ord> OutputCheckSpec<Input> {
    /// Create with default values for "forAllOutputs" case (ignoreSelfRefs = true)
    pub fn default_for_all() -> Self {
        OutputCheckSpec {
            ignore_self_refs: true,
            max_size: None,
            max_closure_size: None,
            allowed_references: None,
            allowed_requisites: None,
            disallowed_references: BTreeSet::new(),
            disallowed_requisites: BTreeSet::new(),
        }
    }

    /// Create with default values for per-output case (ignoreSelfRefs = false)
    pub fn default_per_output() -> Self {
        OutputCheckSpec {
            ignore_self_refs: false,
            max_size: None,
            max_closure_size: None,
            allowed_references: None,
            allowed_requisites: None,
            disallowed_references: BTreeSet::new(),
            disallowed_requisites: BTreeSet::new(),
        }
    }
}

impl<Input: Ord> Default for OutputCheckSpec<Input> {
    fn default() -> Self {
        // Default uses ignoreSelfRefs = true (matching forAllOutputs default)
        Self::default_for_all()
    }
}

/// Output checks - either one set for all outputs or per-output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    bound(
        serialize = "Input: Serialize + Ord",
        deserialize = "Input: Deserialize<'de> + Ord"
    )
)]
pub enum OutputChecks<Input: Ord> {
    /// Output checks that apply to all outputs of the derivation.
    ForAllOutputs(OutputCheckSpec<Input>),
    /// Output checks specified individually for each output.
    PerOutput(BTreeMap<OutputName, OutputCheckSpec<Input>>),
}

impl<Input: Ord> Default for OutputChecks<Input> {
    fn default() -> Self {
        OutputChecks::ForAllOutputs(OutputCheckSpec::default())
    }
}

/// Derivation build-time options.
///
/// These options control various aspects of how a derivation is built,
/// including sandboxing, reference checking, and build locality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    bound(
        serialize = "Input: Serialize + Ord",
        deserialize = "Input: Deserialize<'de> + Ord"
    )
)]
pub struct DerivationOptions<Input: Ord> {
    /// Constraints on what the derivation's outputs can and cannot reference.
    pub output_checks: OutputChecks<Input>,

    /// A map specifying which references should be unsafely discarded from each output.
    #[serde(default)]
    pub unsafe_discard_references: BTreeMap<OutputName, Vec<String>>,

    /// List of environment variable names whose values should be passed as files.
    #[serde(default)]
    pub pass_as_file: BTreeSet<String>,

    /// Specify paths whose references graph should be exported to files.
    #[serde(default)]
    pub export_references_graph: BTreeMap<String, BTreeSet<Input>>,

    /// Additional sandbox profile directives (macOS specific).
    #[serde(default)]
    pub additional_sandbox_profile: String,

    /// Whether to disable the build sandbox, if allowed.
    #[serde(default)]
    pub no_chroot: bool,

    /// List of host paths that the build can access.
    #[serde(default)]
    pub impure_host_deps: BTreeSet<String>,

    /// List of environment variable names that should be passed through from the calling environment.
    #[serde(default)]
    pub impure_env_vars: BTreeSet<String>,

    /// Whether the build should have access to local network (macOS specific).
    #[serde(default)]
    pub allow_local_networking: bool,

    /// List of system features required to build this derivation.
    #[serde(default)]
    pub required_system_features: BTreeSet<String>,

    /// Whether this derivation should preferably be built locally.
    #[serde(default)]
    pub prefer_local_build: bool,

    /// Whether substituting from other stores should be allowed.
    #[serde(default = "default_allow_substitutes")]
    pub allow_substitutes: bool,
}

fn default_allow_substitutes() -> bool {
    true
}

impl<Input: Ord> Default for DerivationOptions<Input> {
    fn default() -> Self {
        DerivationOptions {
            output_checks: OutputChecks::default(),
            unsafe_discard_references: BTreeMap::new(),
            pass_as_file: BTreeSet::new(),
            export_references_graph: BTreeMap::new(),
            additional_sandbox_profile: String::new(),
            no_chroot: false,
            impure_host_deps: BTreeSet::new(),
            impure_env_vars: BTreeSet::new(),
            allow_local_networking: false,
            required_system_features: BTreeSet::new(),
            prefer_local_build: false,
            allow_substitutes: true,
        }
    }
}

/// Type alias for derivation options with resolved store paths.
pub type BasicDerivationOptions = DerivationOptions<StorePath>;

/// Type alias for derivation options with deriving paths (for unresolved derivations).
pub type FullDerivationOptions = DerivationOptions<SingleDerivedPath>;
