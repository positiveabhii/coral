use coral_api::v1::{PaginationResponse, SearchValuesResponse, Table, TableSummary};
use serde_json::{Map, Value, json};

pub(crate) fn queryable_table_summary_value(table: &TableSummary) -> Value {
    json!({
        "schema_name": table.schema_name,
        "table_name": table.name,
        "name": format!("{}.{}", table.schema_name, table.name),
        "sql_reference": format_schema_table_equivalent(&table.schema_name, &table.name),
        "description": table.description,
        "guide": table.guide,
        "required_filters": table.required_filters,
    })
}

pub(crate) fn missing_table_summary_value(table: &TableSummary) -> Value {
    json!({
        "schema_name": table.schema_name,
        "table_name": table.name,
        "name": format!("{}.{}", table.schema_name, table.name),
        "description": table.description,
        "required_filters": table.required_filters,
    })
}

pub(crate) fn queryable_table_summary_values(tables: &[TableSummary]) -> Vec<Value> {
    let mut summaries = tables
        .iter()
        .map(queryable_table_summary_value)
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    summaries
}

pub(crate) fn table_to_summary(table: &Table) -> TableSummary {
    TableSummary {
        workspace: table.workspace.clone(),
        schema_name: table.schema_name.clone(),
        name: table.name.clone(),
        description: table.description.clone(),
        required_filters: table.required_filters.clone(),
        guide: table.guide.clone(),
    }
}

pub(crate) fn paged_collection_value(
    collection_key: &str,
    items: Vec<Value>,
    pagination: &PaginationResponse,
) -> Value {
    let mut value = Map::from_iter([(collection_key.to_string(), Value::Array(items))]);
    insert_pagination_fields(&mut value, pagination);
    Value::Object(value)
}

pub(crate) fn insert_pagination_fields(
    value: &mut Map<String, Value>,
    pagination: &PaginationResponse,
) {
    value.insert("total".to_string(), json!(pagination.total_count));
    value.insert("limit".to_string(), json!(pagination.limit));
    value.insert("offset".to_string(), json!(pagination.offset));
    value.insert("has_more".to_string(), json!(pagination.has_more));
    if pagination.has_more {
        value.insert("next_offset".to_string(), json!(pagination.next_offset));
    }
}

pub(crate) fn value_search_value(response: &SearchValuesResponse) -> Value {
    let pagination = response.pagination.unwrap_or_default();
    let matches = value_search_matches(&response.values);
    paged_collection_value("matches", matches, &pagination)
}

struct ValueMatch {
    field: String,
    values: Vec<String>,
    total: u32,
}

fn value_search_matches(values: &[coral_api::v1::ValueSearchResult]) -> Vec<Value> {
    let mut matches = Vec::<ValueMatch>::new();
    for value in values {
        let field = observed_value_field(value);
        let Some(existing) = matches
            .iter_mut()
            .find(|candidate| candidate.field == field)
        else {
            matches.push(ValueMatch {
                field,
                values: vec![value.value.clone()],
                total: value.field_total_count.max(1),
            });
            continue;
        };
        existing.values.push(value.value.clone());
        existing.total = existing.total.max(value.field_total_count);
    }

    matches.into_iter().map(value_match_value).collect()
}

fn value_match_value(candidate: ValueMatch) -> Value {
    let page_count = u32::try_from(candidate.values.len()).unwrap_or(u32::MAX);
    json!({
        "field": candidate.field,
        "values": candidate.values,
        "total": candidate.total.max(page_count),
    })
}

fn observed_value_field(value: &coral_api::v1::ValueSearchResult) -> String {
    format!(
        "{}.{}.{}",
        value.schema_name, value.table_name, value.column_path
    )
}

pub(crate) fn format_schema_table_equivalent(schema_name: &str, table_name: &str) -> String {
    format!(
        "{}.{}",
        quote_identifier_if_needed(schema_name),
        quote_identifier_if_needed(table_name)
    )
}

fn quote_identifier_if_needed(identifier: &str) -> String {
    if identifier_needs_quotes(identifier) {
        format!("\"{}\"", identifier.replace('"', "\"\""))
    } else {
        identifier.to_string()
    }
}

fn identifier_needs_quotes(identifier: &str) -> bool {
    let mut chars = identifier.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return true;
    }
    !chars.all(|char| char.is_ascii_lowercase() || char.is_ascii_digit() || char == '_')
}

#[cfg(test)]
mod tests {
    use coral_api::v1::{PaginationResponse, SearchValuesResponse, ValueSearchResult};
    use serde_json::json;

    use super::value_search_value;

    #[test]
    fn value_search_value_groups_repeated_fields() {
        let response = SearchValuesResponse {
            values: vec![
                value_result("slack", "channels", "name", "coral", 13),
                value_result("slack", "channels", "name", "coral-auth", 13),
                value_result("slack", "channels", "name", "coral-benchmarks", 13),
            ],
            pagination: Some(PaginationResponse {
                total_count: 13,
                limit: 10,
                offset: 0,
                has_more: true,
                next_offset: 10,
            }),
        };

        assert_eq!(
            value_search_value(&response),
            json!({
                "matches": [
                    {
                        "field": "slack.channels.name",
                        "values": ["coral", "coral-auth", "coral-benchmarks"],
                        "total": 13
                    }
                ],
                "total": 13,
                "limit": 10,
                "offset": 0,
                "has_more": true,
                "next_offset": 10
            })
        );
    }

    #[test]
    fn value_search_value_preserves_field_groups_in_result_order() {
        let response = SearchValuesResponse {
            values: vec![
                value_result("linear", "issues", "team_key", "BENCH", 1),
                value_result("linear", "issues", "identifier", "BENCH-424", 2),
                value_result("linear", "issues", "identifier", "BENCH-423", 2),
            ],
            pagination: Some(PaginationResponse {
                total_count: 3,
                limit: 20,
                offset: 0,
                has_more: false,
                next_offset: 20,
            }),
        };

        assert_eq!(
            value_search_value(&response),
            json!({
                "matches": [
                    {
                        "field": "linear.issues.team_key",
                        "values": ["BENCH"],
                        "total": 1
                    },
                    {
                        "field": "linear.issues.identifier",
                        "values": ["BENCH-424", "BENCH-423"],
                        "total": 2
                    }
                ],
                "total": 3,
                "limit": 20,
                "offset": 0,
                "has_more": false
            })
        );
    }

    fn value_result(
        schema_name: &str,
        table_name: &str,
        column_path: &str,
        value: &str,
        field_total_count: u32,
    ) -> ValueSearchResult {
        ValueSearchResult {
            workspace: None,
            schema_name: schema_name.to_string(),
            table_name: table_name.to_string(),
            column_path: column_path.to_string(),
            value: value.to_string(),
            value_truncated: false,
            seen_count: 1,
            first_seen_at: "2026-05-19T14:00:00Z".to_string(),
            last_seen_at: "2026-05-19T14:00:00Z".to_string(),
            field_total_count,
        }
    }
}
