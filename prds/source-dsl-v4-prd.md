# Coral Source DSL v4 PRD

## Summary

Coral source specs currently mix three concerns in one table-oriented authoring
model:

1. API shape: provider objects, operations, and request/response contracts.
2. Runtime binding: how Coral calls the provider API.
3. SQL projection: which tables/functions Coral exposes to DataFusion.

This worked for hand-authored sources, but it has a number of shortcomings:

1. It mixes facts (e.g. which entities an API models) with opinions (e.g. which
   tables should be available, which columns should be available on those
   tables, and so on).
2. It asserts that there is exactly one surface per source. e.g. we can model
   GitHub as either a REST API or a GraphQL API, but not both.

DSL v4 introduces a source-model-oriented intermediate representation (IR) that
describes API entities, operations, surfaces, and bindings. The source model can
be generated cheaply and deterministically from an OpenAPI spec or GraphQL
schema. In future, other surfaces (e.g. MCP server) will also be supported.

SQL tables/functions become a separate projection layer derived from, or
explicitly attached to, the source model.

The first version must stay narrow: keep Coral's current manifest envelope,
inputs, auth, headers, and rate-limit behavior. DSL v4 is additive: current
`dsl_version: 3` manifests continue to accept `tables` and `functions`
unchanged. Only new DSL v4 manifests replace those authoring sections with
source-model sections.

## Goals

- Add a richer source model capable of representing representative REST and
  GraphQL APIs without hard-coded provider logic.
- Update the source-spec schema and parser to accept DSL v4 source-model
  manifests alongside existing manifests.
- Preserve existing `dsl_version: 3` manifests and runtime behavior during the
  migration.
- Introduce an explicit SQL projection model separate from the core source IR.
- Execute a narrow GitHub issues slice end to end through the new source model.
- Add conservative automatic projection derivation only after explicit
  projections work.
- Provide a migration bridge from current HTTP manifests to approximate IR for
  inspection and future migration planning.

## Non-Goals

- Redesign auth, secrets, variables, OAuth, custom authenticators, or source
  install UX.
- Migrate all existing sources in the first implementation wave.
- Build a full OpenAPI importer.
- Build a full GraphQL runtime executor.
- Expose mutating operations as SQL affordances.
- Solve all projection naming, curation, identity lookup, or table-vs-function
  policy questions in the first PR.

## Existing System Context

Relevant crates:

- `crates/coral-spec`: parses and validates source specs. Current parser entry
  points are `parse_source_manifest_yaml` and `parse_source_manifest_value`.
  The typed parse result is `ValidatedSourceManifest`.
- `crates/coral-engine`: compiles validated specs into DataFusion providers.
  The compile dispatch is in `crates/coral-engine/src/backends/mod.rs`.
- `crates/coral-app`: loads installed/bundled source specs, stores source
  variables/secrets, and hands validated specs to the engine.

Current manifest path:

```text
manifest.yaml
  -> coral-spec ValidatedSourceManifest
  -> coral-engine compile_validated_manifest
  -> backend-specific CompiledBackendSource
  -> DataFusion TableProvider / TableFunctionImpl registration
```

Current HTTP manifests mix SQL projection with HTTP binding details:

```yaml
tables:
  - name: issues
    filters: ...
    request: ...
    response: ...
    pagination: ...
    columns: ...

functions:
  - name: search_issues
    args: ...
    request: ...
    response: ...
    pagination: ...
    columns: ...
```

The spike branch currently has a rudimentary source model in
`crates/coral-spec/src/types/source.rs`, a rudimentary projection model in
`crates/coral-spec/src/types/projection.rs`, and runtime spike code in
`crates/coral-engine/src/backends/source_model.rs`. That code is useful evidence
but should not be treated as the final design. DSL v4 implementation should
replace or substantially rewrite these spike modules rather than preserve their
current public shape.

## DSL v4 Manifest Boundary

DSL v4 keeps the current manifest envelope and runtime configuration. The
following shape is illustrative; final choices for the backend selector and
projection placement are called out in Open Questions.

```yaml
dsl_version: 4
name: github
version: 1.0.0
description: ...
backend: source_model # illustrative; see Open Questions

inputs: ...
auth: ... # current shape; do not redesign in this project
base_url: ... # current shape for initial REST support
request_headers: ... # current shape for initial REST support
rate_limit: ... # current shape for initial REST support
test_queries: ...

entities: ...
operations: ...
surfaces: ...
bindings: ...
# Provisional: projections may be inline here or emitted as a separate artifact.
# The key and artifact placement are still open questions.
projections: ...
```

The main author-facing change is:

- Remove `tables` and `functions` from the new format.
- Add `entities`, `operations`, `surfaces`, `bindings`, and eventually
  `projections`.

Auth remains outside the source IR for this project. The runtime may attach
existing auth/header/rate-limit config while compiling REST bindings.

## Source IR Requirements

The source model must describe provider APIs without assuming REST.

### Types

Support:

- Scalars: string, integer, float/number, boolean, ID, timestamp/date-time,
  URI/string-like provider scalars, JSON/any, null.
- Enums and constrained values.
- Object types.
- Input object types.
- Interfaces with fields and implementations.
- Unions.
- Arrays/lists.
- Maps/dictionaries.
- Nullable vs optional semantics.
- Descriptions and deprecation metadata where present.

Object fields must support arguments because GraphQL fields such as
`Repository.issues(first, after, states)` are parameterized relationships, not
plain properties.

### Entities and Identity

Entities should be modeled as provider-domain objects with stable identifiers
when known.

The model needs explicit identity keys. A GitHub issue may be addressable by:

- REST path tuple: owner, repo, issue number.
- GraphQL node ID.
- Provider URLs or other identifiers.

The IR should not assume one identity shape is universal. Binding compilation or
future planning can decide whether a requested operation has enough identity
information or needs a lookup.

Identity support in the first wave is representational only. It must capture
known keys and field paths, but it does not need to plan identity lookups across
surfaces. A sketch of the required shape:

```rust
EntityIdentity {
    entity: TypeId,
    keys: Vec<IdentityKey>,
}

IdentityKey {
    name: String,              // e.g. "node_id" or "repository_issue_number"
    fields: Vec<FieldPath>,    // e.g. ["id"] or ["repository.owner.login", "repository.name", "number"]
}
```

### Operations

Operations are logical API capabilities. Inputs are surface-neutral:

```rust
OperationInput {
    name: String,
    ty: TypeRef,
    required: bool,
    // description/defaults may be added as needed
}
```

An operation input must not know whether a REST binding sends the value through
a path parameter, query parameter, header, cookie, or body. It also must not know
whether a GraphQL binding sends the value as a variable or nested input object.

Request body schemas belong in the model as input object types. The REST binding
serializes a logical input as a JSON body. A GraphQL binding serializes a logical
input as GraphQL variables.

### Operation Results

Core operation results should be semantic. HTTP status codes and media types
belong on HTTP response bindings, not the core operation.

The model must still represent:

- single object results
- list results
- wrapper results such as `{ items, total_count }`
- no-body success results, via semantic output variants or a unit/no-content
  type
- binding-local error schemas where needed

For the DSL v4 wave, do not add a core operation-level error type unless a real
fixture forces it. REST errors live on HTTP response bindings, and GraphQL
errors live on GraphQL binding error policy.

## Surface and Binding Requirements

A surface is one provider protocol endpoint or API surface area, such as
GitHub REST, GitHub GraphQL, or a future MCP server. It describes shared
surface-level facts such as protocol kind and base endpoint. A binding maps one
logical operation onto one surface: REST path/query/body mappings, GraphQL root
field and variables, response extraction, pagination, and protocol-specific
errors. Multiple bindings may implement the same logical operation on different
surfaces.

Surface selection across multiple bindings is out of scope for the first
runtime wave. The first executable path should resolve each projected operation
to one REST binding explicitly.

### REST Binding

REST-specific details belong in HTTP bindings:

- HTTP method
- path template
- path/query/header/cookie parameter mappings
- parameter serialization
- request body mapping and content type
- response body path
- response status/media-type behavior
- pagination

The first executable runtime should support the existing GitHub issue list,
search, and get issue patterns:

- path parameters
- query parameters
- singleton object response
- list response
- wrapped list response via an output path such as `items`
- link/page-size style pagination sufficient for the current spike

Wrapped response execution is in scope for the first REST runtime slice: a
binding can extract rows from a path such as `items`. Automatic derivation of
wrapped-list projections is deferred until after simple explicit projections and
simple derivation work.

### GraphQL Binding

GraphQL support in the first project wave is representational, not a full
executor.

The IR should be able to describe:

- query vs mutation
- root field path
- field argument bindings
- variables
- selection policy
- response data path
- Relay connection pagination
- GraphQL error and partial-data policy

Selection policy records what field selection a binding needs in order to
materialize a projected result. It may start as a static/minimal selection set
for fixtures. GraphQL error policy records how the binding treats GraphQL
responses that contain both `data` and `errors`; the first wave only needs to
preserve the policy, not execute it.

Representative GitHub GraphQL shapes:

- `Query.repository(owner, name).issue(number)`
- `Repository.issues(first, after, states, orderBy)`
- `Query.search(query, type, first, after)` returning a union connection
- `createIssue(input: CreateIssueInput)`
- `addReaction(input: AddReactionInput)`
- `node(id: ID!)`

## Projection Model Requirements

SQL projection is not part of the core API IR. Add a separate projection model
that can be explicit first and derived later.

Projection metadata should include:

- SQL table or table-function name
- referenced operation ID
- output cardinality derived from the referenced operation
- output columns
- required inputs
- optional pushdown inputs
- hidden/internal columns
- visibility/publication status
- diagnostics for skipped or ambiguous projection decisions

Initial projection implementation should be explicit: hand-author projection
artifacts for the GitHub issue list/search/get spike, then teach the runtime to
consume those artifacts.

Automatic derivation comes later and must be deliberately conservative.

## Conservative Projection Derivation Rules

The first deriver should only project trivially-projectable operations:

- operation output is `Entity` or `List<Entity>`
- operation inputs are scalar
- entity fields are scalar or JSON-able object fields
- no unions
- no mutations
- no field arguments
- no identity ambiguity
- no complex nested output policy

Unsupported or ambiguous operations must produce diagnostics and no public
projection. The deriver must not guess its way into hundreds of low-quality
tables.

For this deriver, "identity ambiguity" means an operation requires a structured
entity reference, but no single scalar input or declared identity key can be
mapped cleanly to a required projection filter. "Complex nested output policy"
means a field would require a choice between flattening, JSON serialization,
side-table generation, or dropping data. The first deriver should skip those
cases and emit diagnostics.

Wrapped lists, such as GitHub search responses with `items` plus
`total_count`, should be a later step. Initial behavior may project the item
list while emitting diagnostics for skipped metadata.

Table vs table-function policy is a separate design decision. Until decided,
prefer the existing Coral behavior of table projections with required filters
for read operations.

## Runtime Requirements

The new runtime path should be introduced beside the existing backend path.

Target path:

```text
DSL v4 manifest
  -> SourceModelManifest
  -> explicit or derived ProjectionModel
  -> source-model backend compile
  -> REST binding executor
  -> DataFusion TableProvider registration
```

The runtime should reuse existing HTTP execution machinery where practical:

- current source inputs/secrets/variables
- current auth/header config
- current rate-limit handling
- current JSON fetch and row conversion helpers where they fit

Do not replace the existing v3 HTTP backend while introducing the v4 path.

## Legacy Manifest Adapter

Add an adapter from current `dsl_version: 3` HTTP manifests to approximate IR.

Purpose:

- inspect how existing hand-authored sources map to the new model
- support migration planning
- help runtime plumbing move gradually after explicit v4 execution works

Non-purpose:

- it is not the primary DSL v4 authoring format
- it should not hide lossy conversions
- it is advisory-only in this wave; executability is a future question

Expected mappings:

- tables/functions -> operations
- filters/args -> logical operation inputs
- columns -> projection output fields
- request config -> REST bindings
- response mapping -> binding/projection metadata

Lossy or awkward mappings should emit diagnostics. Snapshot adapter output for
representative sources such as GitHub, Linear, Stripe, and Slack. Adapter output
is intended for snapshots and migration review, not for production runtime
registration in this wave.

## Representative Acceptance Slices

### GitHub REST

The source model must represent:

- list repository issues
- search issues
- get issue
- create issue
- one no-body operation such as lock/unlock issue or a delete-like operation

The executable first REST slice should run:

- list repository issues
- search issues
- get issue

### GitHub GraphQL

The model must represent:

- repository issue get
- repository issues connection/list
- createIssue mutation
- addReaction mutation
- node lookup
- search result union/connection

No full GraphQL runtime is required for the first executable slice.

## Implementation Plan

1. Lock the DSL v4 scope and manifest envelope.
2. Implement richer source-model IR types in `coral-spec`.
3. Add DSL v4 schema and parser support beside existing manifests.
4. Add GitHub REST and GraphQL DSL v4 fixtures.
5. Define GraphQL surface and binding model.
6. Define explicit SQL projection model.
7. Replace hard-coded GitHub issue projections with explicit projection
   artifacts.
8. Update source-model runtime to register explicit projections.
9. Compile source-model REST bindings through existing HTTP execution
   machinery.
10. Run GitHub issue list/search/get end to end through DSL v4.
11. Implement conservative projection derivation for simple read operations.
12. Add wrapped-list projection support and diagnostics.
13. Decide table vs table-function projection policy.
14. Add legacy HTTP manifest to source-model IR adapter.
15. Snapshot legacy adapter output for representative sources.
16. Convert a narrow GitHub issues slice to DSL v4 manifest shape.
17. Document DSL v4 authoring and migration constraints.

## Per-Step Acceptance Criteria

### 1. Scope and Envelope

- Update this PRD and/or a short repo-local design note to state that auth,
  inputs, headers, rate limits, and source install UX remain unchanged.
- State that DSL v4 replaces `tables`/`functions` only for new v4 manifests;
  v3 manifests keep their current shape and runtime path.
- Record the chosen manifest selector (`backend: source_model` or an
  alternative) if that decision is made during the step.

### 2. IR Types

- Add serde-friendly Rust structs/enums for the richer source model.
- Round-trip representative REST and GraphQL model fragments.
- Validate basic invariants: unique IDs, valid references, supported type
  shapes, operation/binding references.

### 3. Schema and Parser

- Existing parser tests and source specs keep passing.
- DSL v4 manifests parse into a new validated source-model variant.
- Schema errors identify malformed model sections clearly.

### 4. Fixtures

- Add hand-authored GitHub REST and GraphQL fixtures.
- Fixtures validate without runtime execution.
- Unsupported/deferred constructs produce diagnostics.

### 5. GraphQL Binding Model

- GitHub GraphQL fixtures can represent query/mutation paths, variables,
  field arguments, selection policy, error policy, and Relay connections.
- No production executor is required.

### 6. Projection Model

- Add explicit projection structs separate from core IR.
- Represent the current GitHub issue list/search/get projections.
- Record projection diagnostics.

### 7. Explicit GitHub Projections

- Replace Rust helper constructors for spike projections with explicit
  projection artifacts.
- No automatic projection derivation yet.

### 8. Runtime Projection Registration

- Runtime registers DataFusion tables from explicit projection metadata.
- Errors reference source schema, projection/table name, and missing logical
  input.
- Existing v3 runtime remains unchanged.

### 9. REST Binding Execution

- Source-model REST bindings execute through existing HTTP machinery.
- Tests cover path params, query params, list response, singleton response, and
  wrapped response path.
- This step proves wrapped response extraction for explicit projections; it does
  not derive wrapped-list projections automatically.

### 10. GitHub End-to-End Slice

- Queries equivalent to current spike pass for list/search/get issue.
- Source-level auth/input config keeps current shape.
- The slice remains narrow.

### 11. Conservative Derivation

- Simple list/get operations can derive deterministic projections per
  Conservative Projection Derivation Rules.
- Until the table vs table-function policy is decided, derived read projections
  use table projections with required filters.
- Ambiguous or unsupported operations are skipped with diagnostics.

### 12. Wrapped Lists

- GitHub issue search can be derived from a wrapped `items` response.
- Skipped wrapper metadata is reported.

### 13. Projection Policy

- Produce a written decision for table vs table-function behavior.
- Cover required inputs, search operations, singleton gets, and mutations.

### 14. Legacy Adapter

- Existing HTTP manifests can produce approximate IR without source changes.
- Lossy mappings emit diagnostics.

### 15. Adapter Snapshots

- Snapshot output for at least GitHub, Linear, Stripe, and Slack.
- Snapshots include diagnostics.

### 16. GitHub Slice Manifest

- Convert only the narrow issue slice to DSL v4 manifest shape.
- Do not migrate all GitHub endpoints.

### 17. Docs

- Document DSL v4 authoring, non-goals, limitations, examples, and migration
  constraints.

## Open Questions

- What exact syntax should DSL v4 use for projections: inline under
  `projections`, generated artifact, or both?
- Should initial source-model backend use `backend: source_model`, or should
  `dsl_version: 4` imply the backend shape?
- How should generated/derived IR and projection artifacts be stored for
  bundled sources?
- What is the long-term policy for mutating operations: table functions,
  action-specific tools, approval-gated workflows, or something else?
- How much of existing HTTP response column-expression machinery should survive
  as projection mapping versus move into source IR?

## Fresh-Agent Handoff Notes

A fresh agent starting from `main` should first inspect:

- `crates/coral-spec/src/parser.rs`
- `crates/coral-spec/src/backends/http.rs`
- `crates/coral-spec/src/common.rs`
- `crates/coral-spec/src/schema/source_manifest.schema.json`
- `crates/coral-engine/src/backends/mod.rs`
- `crates/coral-engine/src/backends/http/`
- `crates/coral-engine/src/runtime/registry.rs`
- `sources/core/github/manifest.yaml`

If working from the spike branch, also inspect:

- `crates/coral-spec/src/types/source.rs`
- `crates/coral-spec/src/types/projection.rs`
- `crates/coral-engine/src/backends/source_model.rs`

Before making Rust changes, check the nearest `AGENTS.md`. For Rust changes in
this repo, run `make rust-checks` before submitting a PR.
