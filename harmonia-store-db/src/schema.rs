// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Database schema definitions for the Nix store.
//!
//! Based on Nix's schema.sql and ca-specific-schema.sql (schema version 10).

/// Core schema SQL (ValidPaths, Refs, DerivationOutputs)
pub const SCHEMA_SQL: &str = r#"
create table if not exists ValidPaths (
    id               integer primary key autoincrement not null,
    path             text unique not null,
    hash             text not null,
    registrationTime integer not null,
    deriver          text,
    narSize          integer,
    ultimate         integer,
    sigs             text,
    ca               text
);

create table if not exists Refs (
    referrer  integer not null,
    reference integer not null,
    primary key (referrer, reference),
    foreign key (referrer) references ValidPaths(id) on delete cascade,
    foreign key (reference) references ValidPaths(id) on delete restrict
);

create index if not exists IndexReferrer on Refs(referrer);
create index if not exists IndexReference on Refs(reference);

create trigger if not exists DeleteSelfRefs before delete on ValidPaths
  begin
    delete from Refs where referrer = old.id and reference = old.id;
  end;

create table if not exists DerivationOutputs (
    drv  integer not null,
    id   text not null,
    path text not null,
    primary key (drv, id),
    foreign key (drv) references ValidPaths(id) on delete cascade
);

create index if not exists IndexDerivationOutputs on DerivationOutputs(path);
"#;

/// Content-addressed derivations schema (BuildTraceV3).
///
/// Upstream Nix versions the CA tables independently (Realisations →
/// BuildTraceV3) so users can switch experiment versions without migration; we
/// only target the current revision.
pub const CA_SCHEMA_SQL: &str = r#"
create table if not exists BuildTraceV3 (
    id integer primary key autoincrement not null,
    drvPath text not null,
    outputName text not null,
    outputPath text not null,
    signatures text
);

create index if not exists IndexBuildTraceV3 on BuildTraceV3(drvPath, outputName);
"#;

/// Schema version (matches Nix 2.0+)
pub const SCHEMA_VERSION: i32 = 10;
