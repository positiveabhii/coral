# Coral Identity PRD

## Summary

Coral needs an identity model that is orthogonal to source specs. Sources define
what can be queried. Identities define who or what Coral can act as. They meet
only inside a workspace, where a workspace source assigns a workspace-owned
identity to each source surface that needs one.

The model is:

```text
Source spec       -> materialized source
Identity spec     -> workspace identity
Workspace source  -> a workspace's use of a materialized source
Assignment        -> workspace source surface uses workspace identity
Availability      -> who may query the workspace source
```

There is no global `Connection` object, no source-owned identity, no query-time
identity resolver, and no first-wave per-identity ACL. Those concepts either
duplicate workspace source assignment or create a second permission model.

Users should experience this as a simple authentication UX: source, connected
as, available to, status, permissions needed, affected sources, and fix. Users
should not manage credential material, injection methods, or source IR during
ordinary setup and recovery.

## Goals

- Define identities as workspace-owned materialized objects.
- Define identity specs as the identity-side equivalent of source specs.
- Add `identity_requirements` to DSL v4 surfaces.
- Bind identities through workspace source surface assignments.
- Support private user workspaces, shared workspaces, and local CLI defaults.
- Allow the same source surface to use different identities in different
  workspaces.
- Allow one source to have multiple surfaces, each with its own identity
  requirements.
- Keep matching provider-native: issuer, capabilities, injection method, and
  audience.
- Keep authority changes explicit, especially sharing, reuse, broader
  capabilities, principal changes, and audience changes.

## Non-Goals

- Redesign DSL v4 source IR, projections, OpenAPI import, or source
  materialization internals.
- Make source specs own identity materialization.
- Add query-time identity resolution policy.
- Add cross-workspace identity reuse.
- Add a first-wave per-identity ACL model.
- Normalize provider permissions into a universal Coral taxonomy.
- Define every provider-specific identity spec in this PRD.
- Ship full multi-user sharing in the first local implementation.

## Core Model

### Source Spec

A source spec defines how Coral materializes a queryable source. In DSL v4, it
declares surfaces and projections. Source specs do not choose identities.

### Materialized Source

A materialized source is the result of running a source spec. It contains the
source model, surfaces, and projections that workspaces can use. It is not owned
by one workspace.

### Surface

A surface is a provider interface declared by a source spec, such as GitHub
REST, GitHub GraphQL, Slack Web API, or an AWS service API family. A surface
declares identity requirements. It does not choose a concrete identity.

### Identity Spec

An identity spec defines how Coral materializes provider-facing authority.
Examples:

- `github-oauth`
- `slack-bot-oauth`
- `aws-profile`
- `aws-trusted-role`
- `aws-oidc-trust`
- `manual-bearer-token`

An identity spec owns setup, principal discovery, audience discovery,
capability request or validation, runtime injection method, refresh, recovery,
and supported non-interactive setup.

### Identity

An identity is a workspace-owned, materialized authority created by running an
identity spec inside a workspace.

Examples:

- GitHub OAuth identity for `saul@work`
- Slack bot identity for an Engineering Slack workspace
- AWS STS identity for `arn:aws:sts::123456789012:assumed-role/ReadOnly/saul`
- Google OAuth identity for a Google Workspace user

An identity records non-secret metadata:

- identity spec
- owning workspace
- issuer or authority service
- connected principal
- audience
- capabilities
- supported injection method
- health and recovery state
- credential material reference, when credential material exists

Credential material is an implementation detail. Users should not manage it
directly during normal use.

### Identity Requirements

Identity requirements are declared by source surfaces. They describe what shape
of identity can be assigned to that surface:

- **Issuer / authority service:** who minted or vouches for the identity, such
  as GitHub, Google, Slack, AWS, or Coral OIDC.
- **Capabilities:** provider-native permissions required by the surface, such
  as OAuth scopes, app permissions, IAM actions, or product permissions.
- **Injection method:** how Coral adds the identity to provider requests, such
  as bearer token, API key header, basic auth, or AWS SigV4.
- **Audience constraints:** where the identity must be valid, such as GitHub
  host, Slack workspace, AWS account, AWS partition, region set, Datadog site,
  Google Workspace domain, or provider base URL.

### Workspace

A workspace is the execution and sharing context. It owns identities and
workspace sources. Sources remain independent until a workspace creates a
workspace source from them.

### Workspace Source

A workspace source is a workspace's use of a materialized source. It owns:

- availability
- surface identity assignments
- readiness status for that workspace

### Identity Assignment

An identity assignment is:

```text
workspace source + surface -> workspace identity
```

An assignment is valid only when:

- the identity belongs to the same workspace as the workspace source
- the identity issuer matches an accepted issuer
- the identity capabilities satisfy the surface requirements
- the identity supports the required injection method
- the identity audience satisfies the surface audience constraints

Assignments, not identities, determine where an identity is used.

### Availability

Availability describes who may query a workspace source. It belongs to the
workspace source, not to the source spec or identity.

Availability does not make identities directly reusable. A member uses an
identity only by querying a workspace source whose surface is assigned to that
identity.

## Capability Matching

Capabilities are provider-native facts, not Coral-invented generic
permissions. A universal taxonomy would look tidy and be wrong, because
providers do not expose equivalent semantics.

Matching rules:

- Required capabilities are matched as provider-native facts.
- An identity satisfies a requirement only when Coral can prove the identity has
  that capability or the provider's auth model makes it inherent.
- Unknown, unvalidated, or ambiguous capabilities do not satisfy requirements.
- A stronger capability satisfies a weaker requirement only when the identity
  spec explicitly models that provider-specific implication.
- Requesting additional capabilities is an authority-broadening action and
  requires explicit confirmation.

Specs may still include user-facing labels:

```yaml
capabilities:
  - id: repo:read
    kind: github_app_permission
    label: Repository read access
```

Compatibility uses `id` and provider semantics. UX uses `label`.

## DSL v4 Compatibility

DSL v4 keeps source specs focused on source materialization. Identity adds one
surface-level contract: `identity_requirements`.

Proposed first-wave shape:

```yaml
surfaces:
  - id: github-rest
    type: open-api
    url: https://example.com/github-openapi.yaml
    sha256: ...
    base_url: https://api.github.com
    identity_requirements:
      accepts:
        - id: github-rest-read
          issuer: github
          injection_method: bearer_authorization_header
          audience:
            host: github.com
          capabilities:
            - id: repo:read
              kind: github_permission
              label: Repository read access
            - id: org:read
              kind: github_permission
              label: Organization read access
```

Contract:

- `accepts` has OR semantics.
- Capabilities inside one accepted shape have AND semantics.
- Matching dimensions are issuer, audience, capabilities, and injection method.
- A workspace source assignment chooses a compatible identity owned by the same
  workspace.

DSL v4 projections remain SQL exposure choices. They do not choose identities.

## Examples

### Personal Data Source Across User Workspaces

```text
Materialized source: gmail
Surface: gmail-api

Workspace: saul-private
Identity: google-saul
Workspace source: gmail
gmail.gmail-api -> google-saul
Available to: Saul

Workspace: andrea-private
Identity: google-andrea
Workspace source: gmail
gmail.gmail-api -> google-andrea
Available to: Andrea
```

The same materialized Gmail source is reused. Each user sees their own data
because each user's workspace source assigns the Gmail surface to that user's
own identity. No query-time identity resolver is needed.

### Same Source, Different Workspace Identity

```text
Materialized source: github
Surface: github-rest

Workspace: saul-private
github.github-rest -> github-saul-work

Workspace: eng-shared
github.github-rest -> github-coral-app-acme
```

The source and surface are the same. The workspace assignment is different.

### One Source, Multiple Surfaces

```text
Workspace: eng-shared
Workspace source: github

github.github-rest    -> github-coral-app-acme
github.github-graphql -> github-graphql-oauth-saul
```

Each surface declares its own identity requirements. The workspace assigns a
compatible identity per surface.

## Materialization and Setup

Source and identity materialization are separate flows.

Source materialization follows DSL v4. Running a source spec produces or
refreshes a materialized source with surfaces and projections. It does not
create identities or decide which workspace will use the source.

Identity materialization runs an identity spec inside a workspace. It creates
or refreshes one workspace-owned identity and returns enough non-secret metadata
for UX: connected label, audience, capabilities, status, and recovery action.

Adding a source to a workspace composes the two:

1. Ensure the materialized source exists.
2. Create or select the workspace source.
3. Read identity requirements for each required surface.
4. Find compatible identities already owned by the workspace.
5. Suggest safe reuse only when allowed.
6. Materialize a new identity when needed or chosen.
7. Assign each required surface to a compatible workspace identity.
8. Validate and report ready, partially ready, or blocked.

Setup should use the smallest safe user-visible decision. Compact setup is fine
when there is one obvious private identity path. Reuse, shared access, broader
capabilities, principal changes, audience changes, or availability changes must
show impact before confirmation.

### Non-Interactive Setup

Non-interactive setup is required for tests, benchmarks, CI, MCP-driven flows,
and sandboxed agent environments.

Identity specs may support non-interactive materialization when the provider
allows it safely, such as selecting an existing AWS profile, using configured
cloud trust, or reading a supplied secret reference. Three-legged OAuth may
still require human authorization.

When non-interactive setup cannot proceed safely, Coral should return a
structured, non-secret fix instead of launching a browser or prompting from a
background context:

```text
GitHub identity required.

Workspace source: github
Surface: github-rest
Required access: repo read, org read
Fix: run interactive identity setup for GitHub, then retry.
```

## Query Execution and Recovery

At query time, Coral does not choose identities from a global pool. It uses the
existing workspace source assignment:

1. Resolve the current workspace.
2. Resolve the workspace source for the queried source namespace.
3. Resolve the surface needed by the table or function.
4. Load the workspace identity assigned to that surface.
5. Confirm the identity still satisfies the surface requirements.
6. Inject the identity using the declared injection method.
7. Execute the query.

If no assignment exists, the query is blocked with setup guidance. If the
assigned identity is unhealthy, Coral follows the identity spec recovery rules.
If the identity lacks a required capability, Coral reports an authorization
failure and does not silently request broader access.

Recovery messages should stay workspace-local and non-secret:

```text
GitHub access expired.

Workspace source: github
Surface: github-rest
Connected as: saul@work
Used in this workspace:
  github
Fix: refresh the GitHub identity, then retry.
```

Authorization failures should distinguish access from source health:

```text
Slack denied access to slack.messages.

Workspace source: slack
Surface: slack-web-api
Connected as: Coral Slack App
Reason: message history access has not been granted.
Fix: grant message history access or assign an identity with that capability.
```

## UX Contract

Normal users should see:

- **Source:** what they query.
- **Connected as:** identity assigned in the current workspace.
- **Available to:** who may query the workspace source.
- **Status:** ready, partially ready, blocked, or needs action.
- **Permissions needed:** missing access in provider language.
- **Affected sources/surfaces:** what else in the workspace uses the identity.
- **Fix:** exact next action.

Coral may act silently only when it preserves the same authority:

- refresh credential material using an existing refresh grant
- renew temporary credentials for the same identity spec and audience
- continue using the identity already assigned to the workspace source surface
- revalidate after provider-side recovery

Coral must ask before:

- creating a new identity
- changing principal
- changing audience
- requesting broader capabilities
- changing injection method in a way that changes trust or credential handling
- assigning an identity to another workspace source surface
- sharing a workspace that contains identities or workspace sources
- making a workspace source available to more users
- switching from provider-managed auth to manual token entry

## Sharing, Reuse, and Permissions

Safe defaults:

- New workspaces default to private membership.
- New workspace sources default to private availability.
- New identities belong to the workspace where they are created.
- New identities default to use only by the workspace source that created them.
- Reusing an identity for another workspace source requires explicit selection
  or an allowed suggestion policy.
- Making a workspace source available to more users requires explicit
  confirmation.
- Sharing a workspace requires previewing the workspace sources and identities
  that new members could use.
- Human-owned identities in shared workspace sources require a warning.
- Provider app, bot, service account, trusted-role, or OIDC identities are
  preferred for shared workspace sources.

The first-wave permission model has three permissions, not per-identity ACLs:

- **Query workspace source:** run queries when availability includes the user.
  Query users may see non-secret status, missing permissions, and fix guidance.
- **Manage workspace source:** add or remove a workspace source, create or
  reuse identities for its surfaces, change assignments, recover assigned
  identities, and change availability.
- **Manage workspace:** add or remove members, share the workspace, delegate
  source management, and delete or archive the workspace.

Identity reuse is workspace-local. Coral may suggest an existing identity for
another compatible workspace source surface only when the identity satisfies the
requirements, the audience matches, no broader capability is needed,
availability does not change, and the identity is not limited to its current
workspace source.

Even then, reuse is a choice:

```text
Use existing GitHub identity?

> github-saul-work
  Connected as: saul@work
  Already used by: github
  Access: repo read, org read

  Connect another identity
```

If reuse changes blast radius, Coral must show affected workspace sources.

## Representative Identity Archetypes

The model should fit these archetypes:

- **User OAuth identity:** Google user identity for Gmail, GitHub user OAuth
  identity for GitHub REST.
- **App or bot identity:** Slack bot identity, GitHub App installation identity.
- **Local provider profile identity:** AWS profile resolved through the AWS SDK
  credential chain.
- **Cloud trust identity:** AWS trusted role or AWS OIDC trust.

The first implementation does not need to ship all of them, but it must not
choose concepts that break any of them.

## Acceptance Criteria

- DSL v4 surfaces declare `identity_requirements`; projections do not choose
  identities.
- `coral source add gmail` in a local default workspace can materialize or
  select a Google identity owned by that workspace, create a Gmail workspace
  source, assign `gmail.gmail-api` to that identity, and keep availability
  private.
- The same materialized Gmail source can be used by Saul's and Andrea's private
  workspaces with different workspace-owned Google identities and no query-time
  resolver.
- A shared Engineering workspace can use Slack or GitHub through a
  workspace-owned app, bot, service account, trusted-role, or OIDC identity.
- One workspace identity can be assigned to multiple compatible surfaces in the
  same workspace only after compatibility checks pass and the user confirms any
  increased blast radius.
- Another workspace cannot silently reuse or inherit a workspace-owned identity.
- New workspaces and workspace sources start private; sharing or broadening
  availability previews affected workspace sources and identities.
- Provider-native capability facts drive matching; unknown or unvalidated
  capabilities do not satisfy requirements.
- Background, MCP, CI, and sandboxed flows do not launch interactive auth. They
  use a supported non-interactive path or return a structured fix.
- Expired, unhealthy, or under-permissioned identities produce non-secret,
  workspace-local recovery messages with the workspace source, surface,
  connected identity label, reason, affected scope, and fix.

## Open Questions

- What final DSL v4 field names should replace
  `identity_requirements.accepts`, `issuer`, `injection_method`, `audience`,
  and `capabilities`?
- Which provider-specific `capabilities.kind` values should ship first?
- Which representative identity archetypes should the first implementation ship
  versus validate in fixtures?
- What is the CLI vocabulary: `coral identity`, `coral access`, or another
  surface?
- Which first-wave identity specs support non-interactive materialization?
- What is the minimal local implementation that preserves the workspace-first
  model without shipping full multi-user sharing?
- How should server roles package query, manage workspace source, and manage
  workspace permissions?
