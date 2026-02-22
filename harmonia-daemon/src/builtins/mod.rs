// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Builtin builder implementations.
//!
//! These run in-process instead of spawning an external process,
//! identified by `builder = "builtin:<name>"` in the derivation.

pub mod fetchurl;
pub mod unpack_channel;
