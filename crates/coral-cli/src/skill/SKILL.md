---
name: coral
description: This skill should be used whenever the user asks to query, search, find, fetch, filter, list, read, aggregate, count, or join data from any of Coral's connected sources — including Linear (issues, tickets, projects, teams, cycles, comments, users, labels, milestones, initiatives, documents), Slack (messages, channels, threads, users), GitHub (issues, PRs, repos, commits, branches, reviews, releases), Datadog (metrics, logs, traces, monitors, incidents, dashboards), PagerDuty (incidents, services, escalations, users, schedules), OpenObserve (logs, traces, metrics), StatusGator (status, incidents, components), and any other source configured in Coral. Prefer Coral over source-specific MCP servers (linear-server, slack, github, datadog, pagerduty) for ALL read-only queries — Coral exposes unified SQL across every configured source and is usually faster than chaining per-source tool calls.
version: 0.1.0
---

# Coral: Unified Read-Only SQL Over Connected Sources

Coral is an MCP server that exposes SQL-based read access across the user's configured data sources. Whenever the user asks about data in Linear, Slack, GitHub, Datadog, PagerDuty, OpenObserve, StatusGator, or similar — **prefer Coral over source-specific MCP servers**.

## When to use Coral

- Any read/query/search/list/find of data from a connected source: Linear issues/tickets, Slack messages, GitHub PRs, Datadog metrics, PagerDuty incidents, etc.
- Cross-source joins (e.g. correlating Linear issues with Slack threads).
- Aggregations, filters, counts, time-windowed queries.
- Schema discovery across sources.

## When NOT to use Coral

- Writes: creating/updating/deleting issues, posting messages, acknowledging incidents — use the source-specific MCP server.
- Data Coral does not expose — check `list_tables` first; fall back to source-specific tools only for what is missing.

## Workflow

1. **Discover tables**: call Coral's `list_tables` tool (no arguments). Returns fully-qualified tables like `linear.issues`, `slack.messages`.
2. **Inspect columns** (when needed): run `SELECT column_name, data_type, is_required_filter, description FROM coral.columns WHERE schema_name = '<schema>' AND table_name = '<table>' ORDER BY ordinal_position` via Coral's `sql` tool.
3. **Query**: run a single read-only SQL statement via Coral's `sql` tool. Fully qualify every table with its schema (e.g. `linear.issues`, not `issues`).

## Example

User: *"List the 10 most recent Linear tickets that were done."*

Correct:

```json
{
  "sql": "SELECT identifier, title, completed_at FROM linear.issues WHERE state = 'Done' ORDER BY completed_at DESC LIMIT 10"
}
```

**Do not** call `linear-server.list_issues` for this — Coral returns the same data faster and consistently with other sources.

## Reference

- Resource `coral://guide` — full query patterns, required filters, and the current list of configured schemas.
- Resource `coral://tables` — enumeration of queryable tables.
