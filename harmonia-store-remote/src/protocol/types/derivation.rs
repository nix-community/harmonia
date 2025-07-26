use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use crate::serialization::Serialize;
use harmonia_store_core::{FileIngestionMethod, HashAlgo, StorePath};
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::AsyncWrite;

/// A single derivation output specification
#[derive(Debug, Clone)]
pub struct DerivationOutput {
    /// Path where this output will be placed
    pub path: Option<StorePath>,
    /// How the output hash is computed
    pub hash_algo: Option<HashAlgo>,
    /// Expected hash of the output (base16 encoded)
    pub hash: Option<Vec<u8>>,
    /// Ingestion method for fixed-output derivations
    pub method: Option<FileIngestionMethod>,
}

/// Basic derivation structure for building
/// This is a simplified version that can be sent over the wire
#[derive(Debug, Clone)]
pub struct BasicDerivation {
    /// Derivation outputs (output name -> output spec)
    pub outputs: BTreeMap<Vec<u8>, DerivationOutput>,
    /// Input sources (store paths that must exist)
    pub input_sources: BTreeSet<StorePath>,
    /// Platform/system (e.g. "x86_64-linux")
    pub platform: Vec<u8>,
    /// Builder executable path
    pub builder: Vec<u8>,
    /// Arguments to pass to the builder
    pub args: Vec<Vec<u8>>,
    /// Environment variables for the build
    pub env: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Serialize for DerivationOutput {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Path (empty bytes if None)
        match &self.path {
            Some(path) => path.as_bytes().serialize(writer, version).await?,
            None => (b"" as &[u8]).serialize(writer, version).await?,
        }

        // Hash algo (empty bytes if None)
        match &self.hash_algo {
            Some(algo) => algo.name().as_bytes().serialize(writer, version).await?,
            None => (b"" as &[u8]).serialize(writer, version).await?,
        }

        // Hash (empty bytes if None)
        match &self.hash {
            Some(hash) => hash.serialize(writer, version).await?,
            None => (b"" as &[u8]).serialize(writer, version).await?,
        }

        Ok(())
    }
}

impl Serialize for BasicDerivation {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Number of outputs
        (self.outputs.len() as u64)
            .serialize(writer, version)
            .await?;

        // Each output
        for (name, output) in &self.outputs {
            name.serialize(writer, version).await?;
            output.serialize(writer, version).await?;
        }

        // Input sources
        self.input_sources.serialize(writer, version).await?;

        // Platform
        self.platform.serialize(writer, version).await?;

        // Builder
        self.builder.serialize(writer, version).await?;

        // Args
        (self.args.len() as u64).serialize(writer, version).await?;
        for arg in &self.args {
            arg.serialize(writer, version).await?;
        }

        // Environment
        (self.env.len() as u64).serialize(writer, version).await?;
        for (key, value) in &self.env {
            key.serialize(writer, version).await?;
            value.serialize(writer, version).await?;
        }

        Ok(())
    }
}
