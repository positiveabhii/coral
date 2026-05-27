# Sources Management — v1 (Core) Implementation Plan

**Branch:** `sources-management-v1` (off `main` @ `b8d6f7d`)
**Status:** Core-only scope; community discovery deferred to a follow-up PR (see §3)

## 1. Goal

A real, shippable Sources management page in the Coral UI where a user can:

1. **List** sources installed in their workspace.
2. **Browse / search** the catalogue of **core** (bundled) sources.
3. **Install** a source — credential entry + OAuth flow — backed by real RPCs end-to-end.

Community sources are explicitly **out of scope for this PR**. See §3.

## 2. What ships in this PR

### Backend
- **OAuth token refresh** — cherry-picked from `codex/oauth-token-refresh` (commits `3328047`, `a4d322a`, `b80a3b5`). Required so OAuth source tokens don't bit-rot the moment they expire. Rebases clean once that PR lands upstream.
- **No new RPC surface area.** UI consumes the existing `SourceService` RPCs:
  - `DiscoverSources` — bundled catalog
  - `ListSources` / `GetSource` / `GetSourceInfo` — installed + per-source detail
  - `CreateBundledSource` / `CreateBundledSourceWithOAuth` (streaming) — install
  - `ValidateSource` / `DeleteSource` — detail actions

### UI (`ui/src/`)
- Hash router (`lib/router.ts`) with `traces`, `sources`, `source-install/:name`, `source-detail/:name` routes. Navbar gains a Sources entry with route-aware active state.
- **Sources index** (`views/sources/sources-index.tsx`) — centered max-width-960 layout modelled on `adp/web/src/views/plugins-view.tsx`. Flat alphabetical list with **Connected** above **Available**, both filtered by the same search box. Round provider logo, capitalised name, green Connected pill or grey Core pill in the header, 2-line description, version footer.
- **Install / credential form** (`views/sources/source-install.tsx`) — layout ported from `adp/web/src/components/credential-form.tsx`. Centered max-width-720 page with round logo + capitalised name + Core pill at the top, description below; 2-column grid of fields with sentence-case labels (`API_TOKEN` → "Api token") and manifest `hint` rendered as secondary text; multi-method secrets get a compact segmented control; right-aligned action row with bare Cancel + primary Save.
- **Installed-source detail** (`views/sources/source-detail.tsx`) — read-only surface: configured variables, masked secret keys, Validate (runs the manifest's test queries), Remove behind an inline confirm.
- **Wax dialog primitives** (`wax/components/dialog/*`) — ported from ADP, **not yet used** in v1. Available for the follow-up modal-install pattern if we go that way.

## 3. Community sources — follow-up PR

Per Slack discussion (Kyra / Alberto / James / Martin):

- **Not bundling community metadata at release time.** James explicitly ruled this out.
- **A Coral-hosted API that pulls from GitHub in realtime.** James: *"an API we host that can pull from GH realtime — i.e. we build it"*. Lives off-CLI, on Coral's infra.
- **Docs are fine in the interim.** `withcoral.com/docs/reference/community-sources` — download + `coral source add --file`.
- **Promotion to core is a separate, manual editorial decision.** Some community sources will likely be promoted (databricks, dbt, google drive, figma named in chat) but not as part of this PR.
- **The follow-up PR will NOT include the manifest in the discovery RPC response.** Server-side install, not a YAML round-trip.

The follow-up PR depends on the hosted API existing. Shape it'll take:

```
Local Coral CLI                 Coral-hosted community API
─────────────────              ─────────────────────────────
DiscoverCommunitySources ──→  GET /v1/community → summaries
InstallCommunitySource    ──→  GET /v1/community/:name → manifest
                                (server-side install; manifest
                                 stays server-side, UI never sees it)
```

## 4. Architectural decisions for v1
- **OAuth browser open (Q2)** — UI calls `window.open` inside the streaming-response handler (popup blockers usually allow it since the click stack is still active). Visible fallback link covers blocked-popup case. Strict CLI parity (app spawns browser) would need a `client_opens_browser` hint on the OAuth retrieval request; tracked as a follow-up, not blocking.
- **OAuth token refresh (Q3)** — cherry-picked onto this branch. Rebases clean when `codex/oauth-token-refresh` lands upstream.
- **Loopback completion redirect (Q4)** — current completion page is fine.

## 5. What's NOT in this PR (explicit)
- Community sources discovery / install (see §3).
- Custom source authoring (the "studio" / paste-a-spec flow from the earlier prototype). Will be re-tackled when backend manifest parsing is real.
- Schema explorer surface. Kept on its prototype branch.
- Sources marketplace / ratings / install counts.
- Icon field in the source spec — UI maps a small set of known names to local SVGs; everything else falls back to a plug glyph.
- `outbound_hosts` host-confirmation UX — proto field doesn't exist on `main` (lives on `claude/elastic-brown-8ad186`). Surface on the install + detail pages when that lands.

## 6. Quick map of source files
**Backend (touched only by OAuth refresh cherry-pick):**
- `crates/coral-app/src/credentials/{oauth.rs,mod.rs,store.rs}`
- `crates/coral-app/src/query/manager.rs`
- `crates/coral-spec/src/inputs.rs`
- `crates/coral-app/tests/grpc/oauth_refresh_tests.rs` (new)

**UI (the bulk of this PR):**
- `ui/src/App.tsx`, `ui/src/lib/router.ts` — routing
- `ui/src/components/navbar/navbar.tsx` — Sources nav entry
- `ui/src/lib/{sources,coral-clients,provider-icons}.ts` — typed API + transport + icon map
- `ui/src/views/sources/{sources-index,source-install,source-detail}.tsx` + their `.css.ts` — the three views
- `ui/src/components/{page-header,error-banner,toast}.tsx` — small primitives
- `ui/src/wax/components/dialog/*` — Dialog primitives (ported, unused)
- `ui/src/wax/components/icon.tsx` — extended `IconName` for the icons we use
- `ui/src/utils/to-sentence-case.ts` — credential-form label helper
- `ui/public/images/providers/*` — provider logos

## 7. Verification
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p coral-app` — 138 lib tests + 56 gRPC integration tests
- `npm run build` (type-check + Vite) clean
- `npx oxlint --deny-warnings` clean
- Manual: `cd ui && npm run dev:local`, open `http://localhost:5173/#/sources`. Install Slack via Paste, via OAuth (`SLACK_OAUTH_CLIENT_ID`). Validate + delete from detail page.
