//! Adapters from neutral source-model projections into engine types.

#![allow(
    dead_code,
    reason = "Source-model projection adapter is spike plumbing that is not wired into runtime registration yet."
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use coral_spec::types::projection::{ProjectionFilter, ProjectionScalarType, TableProjection};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};

#[derive(Debug, Clone)]
pub(crate) struct ProjectionTableShape {
    pub(crate) schema: SchemaRef,
    pub(crate) output_columns: Vec<String>,
    pub(crate) filter_only_columns: Vec<String>,
    pub(crate) required_filters: Vec<String>,
}

pub(crate) fn table_shape_from_projection(projection: &TableProjection) -> ProjectionTableShape {
    let output_columns = projection
        .columns()
        .iter()
        .map(|column| column.name().to_string())
        .collect::<Vec<_>>();
    let output_column_set = output_columns.iter().cloned().collect::<BTreeSet<_>>();

    let mut fields = projection
        .columns()
        .iter()
        .map(|column| {
            Field::new(
                column.name(),
                data_type_for_projection_type(column.ty()),
                column.nullable(),
            )
        })
        .collect::<Vec<_>>();

    let mut filter_only_columns = Vec::new();
    for filter in projection.filters() {
        if output_column_set.contains(filter.name()) {
            continue;
        }

        filter_only_columns.push(filter.name().to_string());
        fields.push(field_for_filter(filter));
    }

    let required_filters = projection
        .required_filter_names()
        .into_iter()
        .map(str::to_string)
        .collect();

    ProjectionTableShape {
        schema: Arc::new(Schema::new(fields)),
        output_columns,
        filter_only_columns,
        required_filters,
    }
}

fn field_for_filter(filter: &ProjectionFilter) -> Field {
    Field::new(
        filter.name(),
        data_type_for_projection_type(filter.ty()),
        true,
    )
}

fn data_type_for_projection_type(ty: &ProjectionScalarType) -> DataType {
    match ty {
        ProjectionScalarType::String => DataType::Utf8,
        ProjectionScalarType::Integer => DataType::Int64,
    }
}

#[cfg(test)]
mod tests {
    use coral_spec::types::projection::github_issues_projection;

    use super::*;

    #[test]
    fn builds_arrow_schema_from_github_issues_projection() {
        let projection = github_issues_projection();
        let shape = table_shape_from_projection(&projection);
        let fields = shape.schema.fields();

        assert_eq!(
            fields
                .iter()
                .map(|field| (field.name().as_str(), field.data_type()))
                .collect::<Vec<_>>(),
            vec![
                ("number", &DataType::Int64),
                ("title", &DataType::Utf8),
                ("state", &DataType::Utf8),
                ("created_at", &DataType::Utf8),
                ("user__login", &DataType::Utf8),
                ("owner", &DataType::Utf8),
                ("repo", &DataType::Utf8),
            ]
        );
    }

    #[test]
    fn preserves_projection_scan_metadata() {
        let projection = github_issues_projection();
        let shape = table_shape_from_projection(&projection);

        assert_eq!(
            shape.output_columns,
            vec!["number", "title", "state", "created_at", "user__login"]
        );
        assert_eq!(shape.filter_only_columns, vec!["owner", "repo"]);
        assert_eq!(shape.required_filters, vec!["owner", "repo"]);
    }
}
