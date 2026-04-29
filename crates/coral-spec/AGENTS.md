# AGENTS.md

## Purpose

`coral-spec` owns the declarative source-spec DSL: parsing, validation, input
discovery, and normalized source-definition models.

## Owns

- source-spec structs and enums shared across source kinds
- file and HTTP source-spec parsing
- source-spec validation helpers
- install/import-time input discovery

## Does Not Own

- runtime registration or SQL execution
- app bootstrap, source CRUD, or persistence policy
- CLI prompting or user-facing rendering
- transport or protobuf contracts

## Invariants

- Keep source-spec types transport-neutral. Generated source-manifest protobuf
  types are semantic contract types and may be used here; do not import gRPC or
  transport-layer types.
- Keep runtime execution concerns out of this crate. Engine behavior belongs in
  `coral-engine`.
- Prefer normalized source-spec values over raw YAML plumbing in public
  helpers.
