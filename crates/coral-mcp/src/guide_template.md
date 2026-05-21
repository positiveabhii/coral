# Coral SQL Guide

{{SOURCES_SECTION}}

## Discovery Workflow

Always inspect queryable relations, source-scoped functions, write capabilities, and function effects before writing SQL. Call table functions from `FROM` with named arguments, for example `github.search_issues(q => 'repo:withcoral/coral deploy failure')`.

```sql
-- List visible relations, descriptions, write capabilities, and required filters
SELECT schema_name, relation_name, description, supports_insert, supports_update, supports_delete, required_filters
FROM coral.relations
ORDER BY schema_name, relation_name;

-- List source-scoped table functions, such as provider-native search
SELECT schema_name, function_name, description, effect, idempotency, arguments_json, result_columns_json
FROM coral.functions
ORDER BY schema_name, function_name;

-- Inspect columns for one visible relation, including nullability and write metadata
{{COLUMNS_EXAMPLE}}
```

## Per-Source Configuration

Per-source config values (e.g. Datadog site, Sentry org slug, GitHub API base URL) are exposed via `coral.inputs`. Use it to compose absolute URLs or account-scoped identifiers from source variables. Secret values are never exposed — secret rows always have `value IS NULL`, but `is_set` tells you whether the secret is configured.

```sql
-- Look up a variable value
SELECT value FROM coral.inputs
WHERE schema_name = 'datadog' AND kind = 'variable' AND key = 'DD_SITE';

-- Check which secrets are configured (without revealing values)
SELECT schema_name, key FROM coral.inputs
WHERE kind = 'secret' AND is_set;
```

## JSON Columns

Some source relations expose JSON payloads as `Utf8` columns. Extract fields with the `json_*` functions — path segments are variadic, e.g. `json_get(payload, 'user', 'id')`.

- `json_get(json, path…)` returns a union. Casting to `Boolean`, `Int32/64`, `Float32/64`, or `Utf8` is rewritten to the matching typed function; casts to `Decimal*` stay on the normal cast path and preserve the requested precision/scale.
- Typed shortcuts: `json_get_bool`, `json_get_int`, `json_get_float`, `json_get_str` return the named type directly and yield NULL when the path is missing or the shape doesn't match.
- `json_get_json` returns nested JSON as text for further extraction; `json_get_array` returns `List<Utf8>` where each element is JSON text. String array elements therefore include JSON quotes, e.g. `["\"phoebe-org\""]`. For plain string comparisons, prefer `json_get_str(json, ..., <index>)` when the index is known, or compare against JSON text.
- `json_as_text` renders any value as text (scalars as their text form, objects/arrays as JSON).
- `json_contains` tests path existence; `json_length` returns array/object size; `json_object_keys` lists keys.

```sql
SELECT json_get_str(payload, 'event')              AS event,
       json_get(payload, 'user', 'id')::bigint     AS user_id,
       json_get(payload, 'amount')::decimal(18, 2) AS amount
FROM source.events;
```

```sql
-- json_get_array returns JSON text elements, so string values include quotes.
SELECT *
FROM launchdarkly.flag_environments
WHERE json_get_str(rules, 0, 'clauses', 0, 'values', 0) = 'phoebe-org';
```

## Query Guidance

- Use each relation's `sql_reference` from `list_relations` or `coral://relations` in `FROM`, `JOIN`, and DML clauses, for example `slack.messages`.
- Do not quote the whole `schema.relation` string. Write `github.pulls` or `"github"."pulls"`, not `"github.pulls"`.
- Check `coral.relations.required_filters` and `coral.columns.is_required_filter` before querying relations that depend on filter-only inputs.
- Check `coral.relations.supports_*`, `coral.relations.derived_key_columns`, `coral.columns.is_writable`, and `coral.columns.write_required_on_insert` before issuing `INSERT`, `UPDATE`, `DELETE`, or `TRUNCATE`.
- Cross-source joins work with standard SQL after source scans complete.
- Use `LIKE` or `ILIKE` for SQL wildcard matching with `%` and `_`. `SIMILAR TO` uses regex-shaped patterns, so write `.*` instead of `%`, `.` instead of `_`, or escape literal percent/underscore characters as `\%` and `\_`.
- Regex operators such as `~` and `~*` treat `%` and `_` as ordinary literal characters.
- `list_relations` shows queryable fully qualified relations in pages; pass `schema`, `limit`, and `offset` to narrow large catalogs.
- `search_relations` searches relation names, descriptions, guides, and required filters with a Rust regex; use it before broad SQL metadata scans when you know part of the relation name or required filters.
- `describe_relation` returns one compact relation detail with guide text, required filters, write capabilities, and column count; use `coral.columns` when you need full column details.
- `list_columns` lists columns for one relation; pass `pattern`, `required_only`, `limit`, and `offset` to inspect large schemas progressively. Existing relations return paginated `columns` plus `total`, `has_more`, and optional `next_offset`; regex matches add `matched_fields` per column. Missing relations return `found: false` with suggested recovery calls instead of an empty page.
- `coral://relations` shows relation summaries for all installed sources; `coral.relations`, `coral.columns`, `coral.functions`, and `coral.inputs` provide richer SQL metadata.
