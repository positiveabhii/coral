//! SQL parameter binding coverage.

use std::collections::BTreeMap;

use coral_engine::{CoralQuery, SqlParameterValue, SqlParameters};
use serde_json::json;

use crate::harness::{execution_to_rows, test_runtime};

#[tokio::test]
async fn positional_sql_parameters_bind_to_datafusion_placeholders() {
    let params = SqlParameters::Positional(vec![
        SqlParameterValue::Int64(41),
        SqlParameterValue::Utf8("Grace".to_string()),
        SqlParameterValue::Boolean(true),
        SqlParameterValue::Float64(1.5),
    ]);

    let execution = CoralQuery::execute_sql_with_params(
        &[],
        test_runtime(),
        "SELECT $1 + 1 AS n, $2 AS name, $3 AS active, $4 * 2.0 AS score",
        Some(&params),
    )
    .await
    .expect("parameterized query should succeed");

    assert_eq!(
        execution_to_rows(&execution),
        vec![json!({
            "n": 42,
            "name": "Grace",
            "active": true,
            "score": 3.0
        })]
    );
}

#[tokio::test]
async fn named_sql_parameters_bind_without_leading_dollar() {
    let params = SqlParameters::Named(BTreeMap::from([
        (
            "name".to_string(),
            SqlParameterValue::Utf8("Ada".to_string()),
        ),
        ("n".to_string(), SqlParameterValue::Int64(7)),
    ]));

    let execution = CoralQuery::execute_sql_with_params(
        &[],
        test_runtime(),
        "SELECT $name AS name, $n + $n AS doubled",
        Some(&params),
    )
    .await
    .expect("parameterized query should succeed");

    assert_eq!(
        execution_to_rows(&execution),
        vec![json!({
            "name": "Ada",
            "doubled": 14
        })]
    );
}

#[tokio::test]
async fn null_sql_parameter_can_be_cast_to_a_concrete_type() {
    let params = SqlParameters::Positional(vec![SqlParameterValue::Null]);

    let execution = CoralQuery::execute_sql_with_params(
        &[],
        test_runtime(),
        "SELECT COALESCE(CAST($1 AS BIGINT), 0) AS value",
        Some(&params),
    )
    .await
    .expect("parameterized query should succeed");

    assert_eq!(execution_to_rows(&execution), vec![json!({ "value": 0 })]);
}

#[tokio::test]
async fn missing_sql_parameter_returns_deterministic_error() {
    let error = CoralQuery::execute_sql_with_params(
        &[],
        test_runtime(),
        "SELECT $1 AS missing",
        Some(&SqlParameters::Positional(Vec::new())),
    )
    .await
    .expect_err("missing parameter should fail");

    assert!(
        error.to_string().contains("$1") || error.to_string().contains("placeholder"),
        "error should mention placeholder binding, got: {error}"
    );
}
