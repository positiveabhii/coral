use std::collections::BTreeSet;

use sqlparser::ast::{ObjectName, ObjectNamePart, Query, SetExpr, Statement, TableFactor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct ObservedSurface {
    pub(crate) schema_name: String,
    pub(crate) table_name: String,
}

pub(crate) fn infer_observed_surface(sql: &str) -> Option<ObservedSurface> {
    let dialect = GenericDialect {};
    let statements = Parser::parse_sql(&dialect, sql).ok()?;
    if statements.len() != 1 {
        return None;
    }
    let mut surfaces = BTreeSet::new();
    collect_statement_surfaces(statements.first()?, &mut surfaces);
    let mut surfaces = surfaces.into_iter();
    let surface = surfaces.next()?;
    if surfaces.next().is_some() {
        None
    } else {
        Some(surface)
    }
}

fn collect_statement_surfaces(statement: &Statement, surfaces: &mut BTreeSet<ObservedSurface>) {
    if let Statement::Query(query) = statement {
        collect_query_surfaces(query, surfaces);
    }
}

fn collect_query_surfaces(query: &Query, surfaces: &mut BTreeSet<ObservedSurface>) {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            collect_query_surfaces(&cte.query, surfaces);
        }
    }
    collect_set_expr_surfaces(&query.body, surfaces);
}

fn collect_set_expr_surfaces(expr: &SetExpr, surfaces: &mut BTreeSet<ObservedSurface>) {
    match expr {
        SetExpr::Select(select) => {
            for table in &select.from {
                collect_table_factor_surfaces(&table.relation, surfaces);
                for join in &table.joins {
                    collect_table_factor_surfaces(&join.relation, surfaces);
                }
            }
        }
        SetExpr::Query(query) => collect_query_surfaces(query, surfaces),
        SetExpr::SetOperation { left, right, .. } => {
            collect_set_expr_surfaces(left, surfaces);
            collect_set_expr_surfaces(right, surfaces);
        }
        SetExpr::Table(table) => {
            if let (Some(schema_name), Some(table_name)) = (&table.schema_name, &table.table_name) {
                surfaces.insert(ObservedSurface {
                    schema_name: schema_name.clone(),
                    table_name: table_name.clone(),
                });
            }
        }
        SetExpr::Values(_)
        | SetExpr::Insert(_)
        | SetExpr::Update(_)
        | SetExpr::Delete(_)
        | SetExpr::Merge(_) => {}
    }
}

fn collect_table_factor_surfaces(table: &TableFactor, surfaces: &mut BTreeSet<ObservedSurface>) {
    match table {
        TableFactor::Table { name, .. } | TableFactor::Function { name, .. } => {
            if let Some(surface) = surface_from_object_name(name) {
                surfaces.insert(surface);
            }
        }
        TableFactor::Derived { subquery, .. } => collect_query_surfaces(subquery, surfaces),
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => {
            collect_table_factor_surfaces(&table_with_joins.relation, surfaces);
            for join in &table_with_joins.joins {
                collect_table_factor_surfaces(&join.relation, surfaces);
            }
        }
        TableFactor::Pivot { table, .. }
        | TableFactor::Unpivot { table, .. }
        | TableFactor::MatchRecognize { table, .. } => {
            collect_table_factor_surfaces(table, surfaces);
        }
        TableFactor::SemanticView { name, .. } => {
            if let Some(surface) = surface_from_object_name(name) {
                surfaces.insert(surface);
            }
        }
        TableFactor::TableFunction { .. }
        | TableFactor::UNNEST { .. }
        | TableFactor::JsonTable { .. }
        | TableFactor::OpenJsonTable { .. }
        | TableFactor::XmlTable { .. } => {}
    }
}

fn surface_from_object_name(name: &ObjectName) -> Option<ObservedSurface> {
    let parts = name
        .0
        .iter()
        .filter_map(object_name_part_text)
        .collect::<Vec<_>>();
    let table_name = parts.last()?.clone();
    let schema_name = parts.get(parts.len().checked_sub(2)?)?.clone();
    Some(ObservedSurface {
        schema_name,
        table_name,
    })
}

fn object_name_part_text(part: &ObjectNamePart) -> Option<String> {
    part.as_ident().map(|ident| ident.value.clone())
}
