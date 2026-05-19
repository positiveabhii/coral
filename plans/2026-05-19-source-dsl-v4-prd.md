# Coral Source DSL v4 PRD (OpenAPI-First)

## Summary

Coral source specs currently mix three concerns in one table-oriented authoring
model:

1. API shape: provider objects, operations, and request/response contracts.
2. Runtime execution: how Coral calls the provider API.
3. SQL projection: which tables/functions Coral exposes to DataFusion.

This worked for hand-authored sources, but it has a number of shortcomings:

1. It mixes facts (e.g. which entities an API models) with opinions (e.g. which
   tables should be available, which columns should be available on those
   tables, and so on).
2. It asserts that there is exactly one surface per source. e.g. we can model
   GitHub as either a REST API or a GraphQL API, but not both.
3. It forces hand-authoring of operation, entity, and execution details that are
   already authoritatively described in the provider's own API description
   (OpenAPI, GraphQL schema, etc.).

DSL v4 introduces a source-model-oriented intermediate representation (IR) that
describes API surfaces, surface-scoped operations, and surface-scoped imported
entity candidates. The source model is imported from a provider-supplied API
description rather than hand-authored in the manifest. In this document, the
supported description format is OpenAPI; a GraphQL importer is an explicit
fast-follow.

SQL tables/functions become a separate projection layer explicitly attached
to the source model. The projection layer is the primary place a manifest
author makes opinionated choices about what Coral exposes.

The first version must stay narrow: keep Coral's current source identity,
install, input, and test-query envelope. Reuse existing auth, header, and
rate-limit spec shapes where practical, but attach runtime API configuration to
the relevant surface. DSL v4 is additive: current `dsl_version: 3` manifests
continue to accept `tables` and `functions` unchanged. Only new DSL v4
manifests replace those authoring sections with `surfaces` and `projections`.

## Goals

- Add a richer source model capable of representing representative REST APIs
  without hard-coded provider logic.
- Import the source model from an OpenAPI description referenced by the
  manifest. Authors do not hand-author entities, operations, or execution
  details.
- Materialize imported IR to local app state during source install so subsequent
  loads do not require re-fetching or re-compiling the OpenAPI document.
- Update the source-spec schema and parser to accept DSL v4 source-model
  manifests alongside existing manifests.
- Preserve existing `dsl_version: 3` manifests and runtime behavior during the
  migration.
- Introduce an explicit SQL projection model separate from the core source IR.
- Execute a narrow GitHub issues slice end to end through the new source model,
  driven by GitHub's published OpenAPI description.

## Non-Goals for this wave

- Redesign auth, secrets, variables, OAuth, custom authenticators, or source
  install UX.
- Add migration tooling; existing v3 manifests are not converted, and there is
  no adapter from current HTTP manifests to v4 IR.
- Add a GraphQL importer or full GraphQL runtime executor. The IR must be
  expressive enough that a GraphQL importer is a plausible fast-follow, but no
  GraphQL surface is shipped in this wave.
- Expose mutating operations as SQL affordances. Coral is currently read-only.
- Solve all projection naming, curation, identity lookup, or table-vs-function
  policy questions in the first PR.
- Implement automatic projection derivation; this will come later, but for now
  we only support explicit projections in the manifest.

## Explicit Anti-Goals

- **Hand-authored operations, entities, or protocol execution details in the
  manifest.** A v4 manifest must not declare `entities:`, `operations:`, or
  `bindings:` blocks. Anything the runtime needs in that shape comes from an
  importer running over a referenced API description. Manifests describe _which_
  description to import and _which_ projections to expose; they do not redeclare
  the API.
- A v4 manifest must not contain ad-hoc per-endpoint HTTP details (paths,
  methods, parameter mappings, response paths). Those are properties of the
  OpenAPI document.

The only opinionated authoring surfaces in a v4 manifest are:

- the existing source identity/install envelope (`name`, `version`,
  `dsl_version`, `backend`, `description`, inputs, and test queries),
- `surfaces:` — which API description(s) to import and how to call each
  upstream surface,
- `projections:` — which SQL tables/functions to expose and how they map to
  imported surface operations.

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

## DSL v4 Manifest Boundary

DSL v4 keeps the current source identity/install envelope. Runtime API
configuration moves onto the surface that uses it. The first-wave manifest shape
is locked to this form:

```yaml
# Source identity / install envelope
name: github
version: 1.0.0
dsl_version: 4
backend: source_model
description: ...
inputs: [...]
test_queries: [...]

# New in v4
surfaces:
  # Bundled/core source: ships only a pinned surface descriptor.
  # The OpenAPI artifact lives outside the binary
  - id: github-rest
    type: open-api
    url: https://raw.githubusercontent.com/github/rest-api-description/f5d3342150d3748e7307c81639635706f8338a12/descriptions/api.github.com/api.github.com.yaml
    sha256: 7f2a...
    base_url: https://api.github.com
    auth: { ... }
    request_headers: [...]
    rate_limit: { ... }

  # Or, user-installed/community source: any retrieval URL is allowed, but it
  # must carry a sha256 pin.
  # - id: example-rest
  #   type: open-api
  #   url: https://example.com/openapi.yaml
  #   sha256: 7f2a...
  #   base_url: https://api.example.com
  #   auth: { ... }
  #   request_headers: [...]
  #   rate_limit: { ... }

projections:
  - name: issues
    kind: table
    surface: github-rest
    operation: issues/list-for-repo
    columns: [...]
```

The main author-facing changes versus v3:

- Remove `tables` and `functions` from the new format.
- Add `surfaces` and `projections`.
- Move runtime API configuration (`base_url`, `auth`, `request_headers`, and
  `rate_limit`) from the source top level onto the relevant surface.
- Do **not** add author-facing `entities`, `operations`, or `bindings`. Source
  model details are produced by the importer and live in materialized IR, not in
  the manifest.

Auth remains outside the source IR for this project. The runtime may attach
existing auth/header/rate-limit config from the surface while compiling imported
operations. Source-level `inputs` still declare credentials and variables;
surface-level `auth` defines how a given upstream surface applies them.

The v4 runtime selector is explicit: first-wave DSL v4 manifests must set
`backend: source_model`. This keeps source-model compilation opt-in and avoids
overloading `dsl_version` as both schema version and runtime selector while v3
HTTP, JSONL, and Parquet manifests continue to use their existing backend
dispatch.

DSL v4 is additive. Existing `dsl_version: 3` manifests keep their current
`tables` and `functions` sections and keep the existing runtime path. They do
not need source-model sections, and this project must not change their authoring
or execution behavior.

DSL v4 also does not redesign auth, secrets, source variables, request headers,
rate limits, OAuth, or source installation. Source-level inputs/secrets keep the
current declaration and storage model. Runtime API configuration such as
`base_url`, `auth`, `request_headers`, and `rate_limit` is attached to the
surface that uses it. Source-model work replaces the table/function authoring
sections and relocates runtime API configuration under surfaces for new v4
manifests.

## Surfaces and Importers

A surface entry tells Coral _where_ to find an API description, _which_
importer to apply, and how to call that upstream surface at runtime.

For the first wave, only `type: open-api` is supported. The importer:

1. Resolves the description from its pinned surface descriptor (`url` +
   `sha256`, or a local development file with an equivalent content hash).
2. Parses the OpenAPI document.
3. Produces a Source IR (surfaces, operations, types, and entities).
4. Writes materialized IR to app state during source install.
5. Records the source document's content hash / ETag, `fetched_at`, and the
   importer version as fingerprint metadata on the materialized IR entry. This
   fingerprint is consulted only by an explicit refresh; normal loads do not
   re-fetch.

### Input Pinning

OpenAPI inputs must be reproducible. Two users on the same manifest must
import the same IR.

- A surface descriptor must provide a retrieval location and a content hash. The
  retrieval location does **not** need to be immutable; the `sha256` is the
  reproducibility contract. If fetched bytes do not match the hash,
  materialization fails.
- Bundled/core sources ship only pinned surface descriptors in their manifests.
  They do not embed OpenAPI documents or pre-built IR in the Coral binary. When
  the provider does not offer an immutable historical OpenAPI URL, Coral should
  mirror the exact OpenAPI artifact at a Coral-owned artifact URL and pin that
  URL with `sha256`.
- User-installed/community sources may use any `url:`, including a mutable
  provider URL, but must carry `sha256:` alongside it. If upstream changes the
  document, new installs of the old manifest fail instead of silently importing
  a different IR.

The importer refuses to materialize a manifest without a pinned retrieval
descriptor. There is no unpinned URL mode. ETag, Last-Modified, and provider
version labels may be stored as metadata, but they are not authoritative.

### Coral Artifact Mirror

Bundled/core sources should prefer provider-owned immutable OpenAPI URLs when
they exist. When they do not exist, Coral owns the reproducibility problem by
mirroring the exact OpenAPI document outside the binary at a stable artifact
URL. The manifest pins that artifact URL with `sha256`.

The mirror is not a runtime cache and is not embedded in the Coral binary. It is
a distribution artifact for source installation. Users still materialize IR into
their local app state during `source add`; runtime loads only local materialized
IR.

Maintainer update flow:

1. Fetch the provider's current OpenAPI document using maintainer tooling or
   repo-local update metadata, not runtime manifest fields.
2. If needed, store the exact bytes at a Coral-owned artifact URL.
3. Update the bundled manifest's pinned `url` and `sha256`.
4. Materialize IR from the new descriptor and validate all projections.
5. Ship the manifest change only if projection validation passes.

### Import Timing

Normal source materialization happens at source install time:

- **`source add` for bundled/core sources:** when a user installs a bundled
  v4 source such as `github`, Coral fetches the pinned surface URL, verifies
  the `sha256`, imports IR, validates projections, and writes the IR to local
  app state. The pinned URL may be provider-owned and immutable, or a
  Coral-owned mirror when the provider has no suitable immutable URL.
- **`source add --file` for user-installed/community sources:** when a user
  installs a v4 manifest file, Coral fetches its pinned URL (or reads a local
  development file), verifies the content hash, imports IR, validates
  projections, and writes the IR to local app state.

Query-time and source-load-time imports are explicitly disallowed. Loading an
installed source always reads materialized IR. After install, the only path that
re-imports is the explicit refresh command.

### Surface-Scoped Operation IDs

A surface is one upstream interface such as GitHub REST, GitHub GraphQL, or a
future MCP server. Operations are always scoped to exactly one surface. For the
first OpenAPI wave, each imported operation corresponds to one OpenAPI
operation on one OpenAPI surface.

Projections reference operations by `(surface, operation)` rather than by a
global surface-agnostic operation ID:

```yaml
projections:
  - name: issues
    kind: table
    surface: github-rest
    operation: issues/list-for-repo
```

The operation ID only needs to be unique within its surface. It must still be
deterministic because projections are committed in manifests and must survive
reinstall, refresh, CI, and another user's machine.

The OpenAPI importer assigns surface-scoped operation IDs deterministically:

- If the OpenAPI operation has an `operationId`, use it as the operation ID
  after minimal YAML-safe normalization. Do not reinterpret it into a logical
  Coral action name.
- If no `operationId` is present, generate from method + path (e.g.
  `GET /repos/{owner}/{repo}/issues` → `get.repos.owner.repo.issues` or a
  similar deterministic slug — exact algorithm to be fixed in the importer
  PR).
- Duplicates within a surface are a hard import error with diagnostics naming
  the conflicting operations.

Projection authors discover available `(surface, operation)` pairs by
inspecting materialized IR YAML; a dedicated discovery CLI is out of scope for
this wave.

This PRD intentionally does not introduce logical, cross-surface operation
aliases such as `github.issue.list`. A future wave may add a curated abstraction
for multi-surface equivalence or composite projections, but first-wave
operations are imported surface operations.

A GraphQL importer (`type: graphql`) is the planned fast-follow. The Source IR
defined in this wave must be shaped to accept GraphQL constructs (root fields
with arguments, Relay connections, unions/interfaces, selection policy) without
later breaking changes to the IR public surface or cached artifact format.
This is a representational requirement on the IR; no GraphQL importer ships in
this wave.

First-wave runtime execution supports one surface per manifest. The manifest
shape still names the surface explicitly so the projection contract does not
need to change when multiple surfaces are allowed later. Parser validation may
reject `surfaces.len() != 1` until multi-surface runtime selection is designed.

## IR Materialization

On materialization, Coral writes the produced IR to local app state as YAML (or
another stable, human-inspectable serialization). Subsequent loads of the same
source use the materialized IR if it is still valid.

Materialization requirements:

- **Location.** Materialized IR lives under Coral's existing app data location,
  namespaced by workspace, source name, and source version. Do not invent a new
  top-level cache root. Bundled/core sources use the same local materialization
  path after `source add`; they do not ship committed IR artifacts in the
  binary.
- **Lookup key.** Materialized IR entries are looked up by (workspace, source name,
  source version, pinned surface descriptor, importer version). The pinned
  surface descriptor includes the retrieval location and `sha256`, so changing
  the manifest pin makes the old materialized IR unreachable. All components
  are known from the manifest and local state — lookup never requires network
  access.
- **Stored fingerprint.** Each materialized IR entry records the source document's
  content hash, ETag if available, and a `fetched_at` timestamp. This
  fingerprint is metadata used by explicit refresh, not consulted on the
  normal load path.
- **Format.** YAML by default, matching the rest of Coral's source authoring
  surface, so a developer can inspect materialized IR with the same tools used
  for manifests. The materialized IR format is internal: it is not a public authoring
  surface and may evolve.
- **Invalidation.** A materialized IR entry is considered stale when its importer
  version no longer matches the running Coral, when the manifest's surface
  URL/descriptor changes (different lookup key, old entry is simply not
  found), or when the user runs an explicit refresh command. Refresh fetches
  the source document, compares the new content hash against the stored
  fingerprint, and either confirms the entry is current or replaces it.
  There is no automatic upstream-change detection and no time-based expiry
  in the first wave.
- **Offline behavior.** If valid materialized IR exists, source loading must not
  require network access. Materialization during `source add` may require
  network access; later loads must not.
- **Binary size.** Bundled/core source manifests include only pinned surface
  descriptors and projections. OpenAPI documents and materialized IR are not
  embedded in the Coral binary.

Non-requirements for materialized IR in this wave:

- No cross-machine cache sharing protocol beyond Coral-owned artifact URLs for
  source OpenAPI documents.
- No partial / incremental import.
- No GC policy beyond "user can delete the materialized IR directory".

Open questions (materialization):

- What command surfaces a refresh? (e.g. `coral source refresh`.) The command
  must exist; its exact name and flags are open.

## Source IR Requirements

The Source IR is the importer's output. It is not authored directly; it must
nonetheless be expressive enough to represent representative REST and (later)
GraphQL APIs.

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

Object fields must support arguments so that a GraphQL importer can later
represent parameterized relationships such as
`Repository.issues(first, after, states)` without an IR break.

### Entities and Identity

Entities in first-wave IR are imported, surface-scoped entity candidates. They
are useful metadata for describing result shapes and for future projection
derivation, but they are not canonical provider-wide objects. The OpenAPI
importer may infer entity candidates from named component schemas and response
shapes, and those entities belong to the OpenAPI surface that produced them.

The importer must not automatically assert that an entity imported from one
surface is equivalent to an entity imported from another surface. For example,
GitHub REST `issue` and a future GitHub GraphQL `Issue` may represent the same
provider concept, but that equivalence requires explicit curation in a future
abstraction. Identity keys are evidence, not proof of cross-surface identity.

Identity keys are explicit and may be multiple per surface entity. A GitHub REST
issue may be addressable by:

- REST path tuple: owner, repo, issue number.
- Provider URLs or other identifiers.

A future GraphQL surface may import a separate `Issue` entity candidate with a
GraphQL node ID. Mapping those identities onto one canonical logical entity is
out of scope for this wave.

Identity support in the first wave is representational only — the importer
records known keys on surface entities; runtime compilation does not yet plan
identity lookups across surfaces.

```rust
EntityIdentity {
    surface: SurfaceId,
    entity: TypeId,
    keys: Vec<IdentityKey>,
}

IdentityKey {
    name: String,
    fields: Vec<FieldPath>,
}
```

### Operations

Operations are callable or readable capabilities produced by the importer and
scoped to one surface. For OpenAPI in the first wave, the importer produces one
operation per OpenAPI operation. Inputs are operation-level values:

```rust
OperationInput {
    name: String,
    ty: TypeRef,
    required: bool,
}
```

The operation also carries surface-specific execution metadata. For REST
operations, that includes whether each input is serialized as a path parameter,
query parameter, header, cookie, or body. Request body schemas live in the IR as
input object types; the REST operation records how to serialize them.

### Operation Results

Operation results include semantic output shape plus any surface-specific
response metadata needed for execution. For REST operations, HTTP status codes,
media types, response extraction, pagination, and error schemas live on the REST
operation details.

The model must represent:

- single object results
- list results
- wrapper results such as `{ items, total_count }`
- no-body success results, via semantic output variants or a unit/no-content
  type
- surface-local error schemas where needed

For this wave, do not add a core operation-level error type unless a real
fixture forces it. REST errors live on REST operation details.

## Surface and Operation Requirements

A surface (in IR terms) is one provider protocol endpoint or API surface area,
such as GitHub REST. It records protocol kind and base endpoint. Operations
belong to exactly one surface and carry any protocol-specific execution details
needed to call that surface.

Surface selection across multiple surfaces is out of scope for this wave. The
first executable path resolves each projection to one operation on one REST
surface.

### REST Operation (importer output)

REST-specific details produced by the OpenAPI importer:

- HTTP method
- path template
- path/query/header/cookie parameter mappings
- parameter serialization
- request body mapping and content type
- response body path
- response status/media-type behavior
- pagination (link header / page+per_page style as needed for GitHub)

The first executable runtime supports the existing GitHub issue list, search,
and get-issue patterns:

- path parameters
- query parameters
- singleton object response
- list response
- wrapped list response via an output path such as `items`
- link/page-size pagination sufficient for the GitHub slice

Wrapped response execution is in scope for the first REST runtime slice. The
wrapper path (e.g. `items`) is part of the REST operation produced by the importer
from the OpenAPI response schema — it is **not** authored on the projection.
A projection selects the imported result view by referencing `(surface,
operation)`; it does not configure response paths, methods, or other endpoint
details. Automatic derivation of any projection, wrapped-list or otherwise, is
a non-goal for this wave.

### GraphQL Operation (representational only)

No GraphQL importer or executor ships in this wave. The IR must nonetheless be
shaped so a future importer can produce GraphQL operations without breaking the
IR. Representational coverage to keep in mind:

- query vs mutation
- root field path
- field argument mapping
- variables
- selection policy
- response data path
- Relay connection pagination
- GraphQL error and partial-data policy

If a shape from the above list cannot be added without breaking IR consumers,
it should be designed for now rather than deferred.

## Projection Model Requirements

SQL projection is the manifest author's primary opinionated surface. It is
separate from the importer-produced source IR.

A projection entry references one imported surface operation and describes how
that operation appears as SQL.

Projection authors discover surface IDs and operation IDs by materializing IR
from the pinned surface descriptor and inspecting the resulting YAML. A
dedicated projection discovery CLI is out of scope for this wave.

Projection metadata:

- SQL table or table-function name
- referenced surface ID
- referenced operation ID, scoped to that surface
- output cardinality derived from the referenced operation
- output columns
- required inputs
- optional pushdown inputs
- hidden/internal columns
- visibility/publication status

Initial projection implementation is explicit: hand-author projection entries
for the GitHub issue list/search/get slice and teach the runtime to consume
them.

Automatic derivation comes later; it's out of scope for this PRD.

## Runtime Requirements

The new runtime path is introduced beside the existing backend path.

Target path:

```text
DSL v4 installed source
  -> resolve manifest + materialized IR
  -> validate IR lookup key / importer version / projection references
  -> explicit ProjectionModel
  -> source-model backend compile
  -> REST operation executor
  -> DataFusion TableProvider registration
```

The runtime reuses existing HTTP execution machinery where practical:

- current source inputs/secrets/variables
- existing auth/header config shapes, now read from the selected surface
- existing rate-limit handling, now configured on the selected surface
- current JSON fetch and row conversion helpers where they fit

Do not replace the existing v3 HTTP backend while introducing the v4 path.

## Representative Acceptance Slice

### GitHub REST (via OpenAPI import)

Driven by an immutable URL to GitHub's published REST OpenAPI description (i.e.
a URL containing a commit SHA, not "heads/main"), the source model must
represent:

- list repository issues
- search issues
- get issue
- create issue
- one no-body operation such as lock/unlock issue or a delete-like operation

The executable first REST slice runs:

- list repository issues
- search issues
- get issue

The slice exercises two paths:

- Materialization path: `source add github` fetches the pinned surface URL,
  verifies `sha256`, imports IR, validates projections, and writes local
  materialized IR.
- Runtime path: a source with materialized IR registers projections and runs
  DataFusion queries without OpenAPI network access or importer execution.

### GraphQL Fast-Follow

Out of scope for this PRD; tracked as a fast-follow after v4 ships. The IR
shape requirements above exist so the fast-follow does not require an IR
break.

## Implementation Plan

1. Lock the DSL v4 scope and manifest envelope (this PRD).
2. Implement richer source-model IR types in `coral-spec`, with shape that
   accommodates a later GraphQL importer without break.
3. Add DSL v4 schema and parser support beside existing manifests. Manifests
   accept `surfaces` and `projections`; reject `entities`/`operations`/
   `bindings` at the manifest layer.
4. Implement OpenAPI importer producing Source IR for a given, immutable Github
   OpenAPI spec URL.
5. Implement source materialization: `source add` fetches/reads a pinned OpenAPI
   descriptor, verifies `sha256`, imports IR, validates projections, and writes
   materialized IR under app state. Subsequent loads read materialized IR with
   no network access and no importer execution.
6. Define explicit SQL projection model and parse it from manifests.
7. Implement list repository issues, search issues and get issue with explicit
   projection artifacts in a v4 manifest.
8. Update source-model runtime to register explicit projections from imported
   IR.
9. Compile source-model REST operations through existing HTTP execution
   machinery.
10. Run GitHub issue list/search/get end to end through DSL v4 after
    materialization, and separately test the `source add` materialization path.
11. Document DSL v4 authoring, the OpenAPI-only import constraint, and the
    GraphQL fast-follow.

## Per-Step Acceptance Criteria

### 1. Scope and Envelope

- This PRD states the OpenAPI-first scope and the anti-goal on hand-authored
  operations/entities/bindings.
- States the GraphQL importer fast-follow.
- Records the chosen manifest selector (`backend: source_model`, matching
  existing serde/schema/parser identifiers).
- Records the input-pinning model: bundled/core sources ship pinned surface
  descriptors, optionally backed by Coral-owned artifact mirrors; user-installed
  sources require a retrieval location plus `sha256`. No unpinned URLs.
- Records the Coral artifact mirror policy: provider immutable URLs are fine;
  otherwise bundled/core sources pin Coral-owned mirrored OpenAPI artifacts that
  live outside the binary.
- Records the import-timing rule: `source add` materializes IR; no query-time or
  lazy load-time imports.
- Records the surface runtime configuration rule: `base_url`, `auth`,
  `request_headers`, and `rate_limit` are surface-level in v4, while source-level
  inputs still declare credentials and variables.

### 2. IR Types

- Add serde-friendly Rust structs/enums for the richer source model.
- Round-trip representative REST model fragments and at least one synthetic
  GraphQL fragment to demonstrate the IR is not REST-only.
- Validate basic invariants: unique IDs, valid references, supported type
  shapes, and projection references.
- Entity candidates are scoped to the surface that imported them. The first-wave
  IR does not assert canonical cross-surface entity equivalence.

### 3. Schema and Parser

- Existing parser tests and source specs keep passing.
- DSL v4 manifests parse into a new validated source-model variant.
- Schema errors identify malformed `surfaces` and `projections` sections
  clearly.

### 4. OpenAPI Importer

- Imports a GitHub-shaped OpenAPI document into Source IR.
- Covers path/query parameters, body parameters, singleton response, list
  response, and wrapped list response.
- Assigns surface-scoped IR operation IDs deterministically (from `operationId`
  when present, otherwise from method + path). Duplicates within a surface are a
  hard error.
- Refuses inputs that are not pinned (manifest must supply a retrieval location
  plus `sha256`; local development files are allowed only with equivalent hash
  validation).
- Emits diagnostics for unsupported constructs rather than failing the import.
- Has unit tests that do not require network access.

### 5. IR Materialization

- Bundled/core sources do not embed OpenAPI documents or pre-built IR in the
  Coral binary.
- `source add github` fetches the pinned surface URL, verifies `sha256`,
  imports IR, validates projections, and writes materialized IR under app state.
- User-installed/community sources follow the same materialization path from
  their pinned retrieval descriptor.
- Subsequent loads of either source kind perform no OpenAPI network I/O and do
  not run the importer.
- Materialized IR entries are looked up by (workspace, source name, source version,
  pinned surface descriptor, importer version); the descriptor includes the
  retrieval location and `sha256`.
- Importer-version change makes existing entries unreachable on the normal
  load path.
- A user-facing refresh path exists. Refresh re-fetches the source document,
  compares the new content hash to the manifest pin, imports into a temporary
  materialization directory, validates projections, and atomically replaces the
  local IR only if the hash and projections still match. If the hash differs,
  refresh fails and reports the new hash unless an explicit manifest-update
  flow is implemented.

### 6. Projection Model

- Add explicit projection structs separate from core IR.
- Manifest `projections:` block parses into these structs.
- Projections reference operations by stable `(surface, operation)` pairs.

### 7. Explicit GitHub Projections

- Implement explicit projection entries in a v4 manifest.
- No automatic projection derivation yet.

### 8. Runtime Projection Registration

- Runtime registers DataFusion tables from explicit projection metadata
  resolved against imported IR.
- Errors reference source schema, projection/table name, and missing operation
  input.
- Existing v3 runtime remains unchanged.

### 9. REST Operation Execution

- Source-model REST operations produced by the importer execute through existing
  HTTP machinery.
- Tests cover path params, query params, list response, singleton response,
  wrapped response path, and link-header / page+per_page pagination
  sufficient for the GitHub slice.

### 10. GitHub End-to-End Slice

- Queries pass for list/search/get issue, driven by materialized OpenAPI IR.
- Runtime with materialized IR performs no OpenAPI network I/O and no importer
  execution.
- Missing or stale materialized IR fails clearly and points to `source add`,
  reinstall, or refresh.
- Source-level input config keeps the current shape; REST auth, headers, base
  URL, and rate limits are read from the selected surface.

### 11. Docs

- Document DSL v4 authoring, the OpenAPI-only import constraint, the anti-goal
  on hand-authored operations/entities/bindings, materialized IR, surface-level
  runtime configuration, Coral artifact mirror behavior, and the GraphQL
  fast-follow.

## Open Questions

- Exact CLI surface for materialized IR refresh (command name, flags, behavior
  when the remote sha256 differs from the manifest pin).
- Exact slug algorithm for surface-scoped IR operation IDs when OpenAPI
  `operationId` is missing.
- What does the importer do when an OpenAPI document is partially malformed:
  hard fail, skip operations, or import with diagnostics?
- How are multiple surfaces in one manifest reconciled when the GraphQL importer
  lands? First wave assumes exactly one surface per manifest.
- What is the long-term policy for mutating operations: table functions,
  action-specific tools, approval-gated workflows, or something else?

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

Before making Rust changes, check the nearest `AGENTS.md`. For Rust changes in
this repo, run `make rust-checks` before submitting a PR.
