# Sources Management — v1 Implementation Plan

**Branch:** `sources-management-v1` (off `main` @ `b8d6f7d`)
**Status:** Draft for review
**Author:** drafted with Claude Code, 2026-05-25

## 1. Goal

A real, shippable Sources management page in the Coral UI where a user can:

1. **List** sources that are already installed in their workspace.
2. **Browse / search** the catalog of available sources (core + community).
3. **Install** a source — including credential entry and OAuth flows — backed by the real backend, not mocks.

This replaces the prototype on `ludo-sources-review-v2`. Custom-source authoring (the "studio" / "create" flow on the prototype) is **out of scope for v1**.

---

## 2. What the backend gives us today

### Source spec model (`crates/coral-spec`)
- DSL v3 manifest schema at `crates/coral-spec/src/schema/source_manifest.schema.json:7`
- Backends: `http`, `parquet`, `jsonl`, `mcp` (`schema.json:74`)
- Auth on the transport: `BasicAuth`, `HeaderAuth`, `CustomAuth` (`schema.json:257`)
- **Credentials on inputs** — secrets declare `credential.methods`, each either:
  - `source_config` (user pastes the secret), or
  - `oauth` (auth-code only today, with PKCE option, fixed or random loopback redirect)
- Types: `crates/coral-spec/src/inputs.rs:20-200`

### Sources on disk
- **Core (22):** YAML manifests in `sources/core/<name>/`, **compiled into the binary** by `crates/coral-app/build.rs:14-54`
- **Community (75):** YAML manifests in `sources/community/<name>/`, **not bundled**, not exposed at runtime today

### Source service (`crates/coral-api/proto/coral/v1/sources.proto:456-476`)
Already implemented gRPC RPCs:

| RPC | Purpose | Streaming? |
| --- | --- | --- |
| `DiscoverSources` | List bundled sources available to install | no |
| `ListSources` | List installed sources | no |
| `GetSource` / `GetSourceInfo` | Fetch one source | no |
| `CreateBundledSource` | Install a bundled source (no OAuth) | no |
| `CreateBundledSourceWithOAuth` | Install bundled + run app-owned OAuth | **yes** |
| `ImportSource` | Import a user-supplied manifest YAML + run OAuth | **yes** |
| `DeleteSource` | Remove installed source | no |
| `ValidateSource` | Run test queries against an installed source | no |

The streaming OAuth responses emit (`sources.proto:274-310`):
- `oauth_authorization` — authorize URL + state, UI opens it in the browser
- `oauth_completed` — token retrieved, safe metadata available
- `source` — install completed, with installed source resource

This is the key insight: the UI does **not** need to host the OAuth callback. The app already binds a local loopback listener (`crates/coral-app/src/credentials/oauth.rs:299-432`) and exchanges the code itself. The UI just opens the URL and watches the stream.

### OAuth runtime (`crates/coral-app/src/credentials/oauth.rs`)
- Authorize URL construction, state token, PKCE, fixed vs. random loopback ports
- Loopback listener (10 min `SESSION_TTL`)
- Token exchange, plaintext token persistence (`store.rs`), per-source secret env files
- Confirmed outbound hosts statically extracted (`ValidatedSourceManifest::outbound_hosts()`)
- **Token refresh is NOT on main yet** — sits on `codex/oauth-token-refresh` (commits `0abf2fa`, `432a1b8`, `89f4936`). Needs to be merged or cherry-picked before v1 ships.

### UI client
- Connect-style TS bindings generated at `ui/src/generated/coral/v1/sources_pb.ts` — already includes `CreateBundledSourceWithOAuth` and its streaming response shape.

---

## 3. What the prototype gives us

The prototype on `ludo-sources-review-v2` is a UX skeleton with patchy backend integration. Inventory below; full review in agent notes.

### Keep & adapt (high value)
- **`source-install.tsx`** — ~95% production-ready credential form. Real `discoverCatalog()` + `createBundledSource()` calls, secret-toggle inputs, required-field validation, busy/error states. Just needs OAuth wiring.
- **`sources-index.tsx`** layout — Connected section + Library section with facet filters (all / core / community / installed) and client-side search.
- **Router & navigation pattern** (`ui/src/lib/router.ts`) — hash-based, type-safe.
- **`create-seed.ts`** — small scratchpad pattern for handing data between routes without URL coupling. Reusable.
- **Vanilla-extract styling** consistent with `@/wax` design system.

### Defer to v1.1+
- **`source-studio.tsx`** inspect mode — useful as a "view source details" surface but not required for the list/search/install flow.
- **`schema-explorer.tsx`** — nice developer tool, orthogonal to source management. Keep on its branch; not part of v1.

### Drop
- **`source-create.tsx`** — custom source authoring is mocked end-to-end (artificial `pickStudioModel()` delays, fake diagnostics). v1 does not include custom-source authoring.
- **`source-studio.tsx`** create mode — depends on the above.
- **`popular-mocks.ts`**, **`studio-manifests.ts`** — hardcoded mock data; replaced by real backend catalog.

---

## 4. Architectural decisions (locked)

- **Community sources (Q1) → Option C.** Build-time `community_catalog.json` with metadata; manifest YAML fetched on install. New RPC `DiscoverCommunitySources`. Dev: read from local FS; release: fetch from GitHub raw pinned to the build's commit SHA.
- **OAuth browser open (Q2) → same path as CLI.** App spawns the system browser (don't introduce `window.open` in the UI). UI still shows the URL as a copy-able fallback if open fails.
- **OAuth token refresh (Q3) → cherry-picked onto this branch.** Commits `0abf2fa`, `432a1b8`, `89f4936` are already on `sources-management-v1` (as `3328047`, `a4d322a`, `b80a3b5`). When `codex/oauth-token-refresh` merges to main upstream, this branch will rebase clean.
- **Loopback completion redirect (Q4) → skip for v1.** Current completion page is fine.

---

## 5. Implementation plan

### Milestone 0 — Foundations (no merge yet)
- Confirm Q1–Q4 with user; lock decisions.
- Rebase / track `codex/oauth-token-refresh` status.
- Set up the v1 directory structure on this branch:
  - `ui/src/views/sources/` — fresh, lifting only the pieces marked "Keep" above.
  - Don't carry over the prototype WIP commit; re-create what we need clean.

### Milestone 1 — Backend: catalog + community discovery (only if Q1 = B or C)
- New RPC `DiscoverCommunitySources` returning `[CommunitySourceSummary { name, version, description, manifest_url, icon? }]`.
- Build-time generator producing the catalog (`crates/coral-app/build.rs` — extend).
- Optional server-side proxy to fetch manifest YAML when a user clicks install (so we don't hit GitHub from the browser, and we can cache).
- Tests for catalog stability and manifest fetch.

Skip this milestone if Q1 = A (bundle everything) — instead extend `build.rs` to bundle community sources too and `DiscoverSources` already returns them.

### Milestone 2 — Backend: minor polish
- Ensure `SourceInfo` exposes everything the UI needs for browsing: description, version, declared inputs with credential method kinds, declared outbound hosts (already there), and ideally an icon name/slug field — add to spec + `SourceInfo` if missing.
- Verify `CreateBundledSourceWithOAuth` end-to-end against Slack via a test.

### Milestone 3 — UI: Sources index page
- New route `#/sources` and link from primary nav (currently nav only goes to Traces).
- Two sections: **Installed** and **Catalog**.
- Catalog: real data from `DiscoverSources` (+ `DiscoverCommunitySources` per Q1). Facets (core / community / installed), text search, source cards.
- Installed: `ListSources` + per-source validate / view / delete (delete behind a confirm).
- Adapt layout & facet logic from prototype `sources-index.tsx`.

### Milestone 4 — UI: Install flow (non-OAuth)
- Route `#/sources/install/:name`.
- Fetch `GetSourceInfo`, render the input form (variables + secrets).
- For each secret, render a credential-method picker if `>= 2` methods declared (e.g., Slack has OAuth + paste).
- For `source_config` method: existing prototype form is the baseline.
- Submit → `CreateBundledSource`, navigate to installed view on success, show errors inline.

### Milestone 5 — UI: OAuth flow
- When user picks the `oauth` method, submit calls `CreateBundledSourceWithOAuth` (streaming).
- Handle stream events:
  - `oauth_authorization` → `window.open(url, '_blank')` and show a waiting UI ("Complete sign-in in your browser") with the URL as a fallback button.
  - `oauth_completed` → swap waiting UI to "Finishing install…".
  - `source` → navigate to installed view, toast success.
  - error → show inline with the authorize URL so the user can retry.
- Handle the `outbound_hosts` confirmation: show the host list to the user **before** kicking off the install (matches the CLI's behavior in `c4d07ea`).

### Milestone 6 — UI: Installed-source detail (light)
- Click an installed source → side panel or sub-route with: validation status, table count, declared outbound hosts, delete button.
- This is much smaller than the prototype's "studio" — pure read-only summary.

### Milestone 7 — Tests + polish
- Vitest covering the install state machine (esp. OAuth stream handling).
- One e2e (or scripted manual) install of Slack via OAuth against a test workspace.
- Empty / error / loading states across all views.
- Strings & icons.

### Milestone 8 — PR
- One PR or a short stack:
  - PR1: backend additions (catalog + spec fields) — if Q1 needs them
  - PR2: UI sources page + install flow (the bulk)
- Include CHANGELOG entry.
- Manual QA matrix: install bundled + paste-token, install bundled + OAuth (Slack), install community (if in scope), delete, validate, error paths.

---

## 6. Out of scope for v1 (explicit)
- Custom source authoring (the prototype's "create" / "studio" flows). Will be re-tackled when backend manifest parsing is real.
- Schema explorer surface. Keep on its branch; revisit after v1 ships.
- Sources marketplace / ratings / install counts.
- Multi-credential management per source (rotating OAuth identities).
- Sources from a private/internal registry beyond GitHub.

---

## 7. Risks
- **OAuth token refresh not merged yet** — v1 ships broken-ish for OAuth sources without it. Treat its merge as a hard prerequisite.
- **Community source UX hinges on Q1.** Bundling adds binary weight; runtime fetching adds a network dependency and a caching story.
- **Browser popup blockers** can block `window.open` initiated outside a direct click. The install button must be the direct caller of the streaming RPC, and `window.open` must run inside the event handler of the first stream message (or we fall back to "click to open").
- **No icon metadata in spec today** — UX of the catalog needs *some* visual differentiation. Adding `icon` to the spec is small but touches every existing manifest.

---

## 8. Quick map of source files for this work

Backend
- `crates/coral-spec/src/schema/source_manifest.schema.json` — manifest schema
- `crates/coral-spec/src/inputs.rs` — credential method + OAuth types
- `crates/coral-app/build.rs` — bundled source generation
- `crates/coral-app/src/sources/{catalog.rs,service.rs,manager.rs}` — runtime + RPC handlers
- `crates/coral-app/src/credentials/oauth.rs` — OAuth flow
- `crates/coral-api/proto/coral/v1/sources.proto` — RPC surface

UI (to be authored on this branch — lifting selectively from `ludo-sources-review-v2`)
- `ui/src/views/sources/sources-index.tsx`
- `ui/src/views/sources/source-install.tsx`
- `ui/src/lib/router.ts`, `ui/src/App.tsx` — route registration
- `ui/src/generated/coral/v1/sources_pb.ts` — already-generated client

---

## 9. Decisions confirmed — see Section 4.
