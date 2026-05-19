//! Registration-only runtime for DSL v4 source-model projections.
//!
//! This backend resolves explicit SQL projections against materialized
//! importer IR and exposes their schemas to `DataFusion`. REST execution is
//! implemented in the follow-on source-model runtime step.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::catalog::TableFunctionImpl;
use datafusion::datasource::TableProvider;
use datafusion::datasource::empty::EmptyTable;
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::Expr;
use datafusion::prelude::SessionContext;

use crate::CoreError;
use crate::backends::common::RegisteredColumn;
use crate::backends::{
    BackendCompileRequest, BackendRegistration, CompiledBackendSource, RegisteredSource,
    RegisteredTable, RegisteredTableFunction, SourceTableFunctions, build_registered_inputs,
    internal_table_function_name, registered_columns_from_specs, schema_from_columns,
};
use coral_spec::{
    OperationInput, ProjectionKind, SourceModelIr, SourceModelOperation, SourceModelProjection,
    SourceModelSourceManifest,
};

#[derive(Debug, Clone)]
struct SourceModelCompiledSource {
    manifest: SourceModelSourceManifest,
    ir: SourceModelIr,
    source_secrets: BTreeMap<String, String>,
    source_variables: BTreeMap<String, String>,
}

pub(crate) fn compile_manifest(
    manifest: &SourceModelSourceManifest,
    request: &BackendCompileRequest<'_>,
) -> std::result::Result<Box<dyn CompiledBackendSource>, CoreError> {
    let Some(ir) = request.source_model_ir.clone() else {
        return Err(CoreError::FailedPrecondition(format!(
            "source '{}' uses backend=source_model but has no materialized source-model IR; run `coral source refresh {}` or reinstall the source",
            manifest.common.name, manifest.common.name
        )));
    };
    ir.validate(&manifest.projection_refs())
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;

    Ok(Box::new(SourceModelCompiledSource {
        manifest: manifest.clone(),
        ir,
        source_secrets: request.source_secrets.clone(),
        source_variables: request.source_variables.clone(),
    }))
}

#[async_trait]
impl CompiledBackendSource for SourceModelCompiledSource {
    fn schema_name(&self) -> &str {
        &self.manifest.common.name
    }

    fn source_name(&self) -> &str {
        &self.manifest.common.name
    }

    async fn register(&self, _ctx: &SessionContext) -> Result<BackendRegistration> {
        let operations = operations_by_ref(&self.ir);
        let mut tables: HashMap<String, Arc<dyn TableProvider>> = HashMap::new();
        let mut table_infos = Vec::new();
        let mut table_functions = SourceTableFunctions::new();
        let mut table_function_infos = Vec::new();

        for projection in &self.manifest.projections {
            let operation =
                resolve_projection_operation(self.schema_name(), projection, &operations)?;
            match projection.kind {
                ProjectionKind::Table => {
                    let required_inputs = required_operation_inputs(operation);
                    validate_table_projection_inputs(
                        self.schema_name(),
                        projection,
                        operation,
                        &required_inputs,
                    )?;
                    let schema = schema_from_columns(
                        &projection.columns,
                        self.schema_name(),
                        &projection.name,
                    )?;
                    tables.insert(
                        projection.name.clone(),
                        Arc::new(EmptyTable::new(schema)) as Arc<dyn TableProvider>,
                    );
                    table_infos.push(registered_table(projection, &required_inputs));
                }
                ProjectionKind::Function => {
                    let schema = schema_from_columns(
                        &projection.columns,
                        self.schema_name(),
                        &projection.name,
                    )?;
                    let internal_name =
                        internal_table_function_name(self.schema_name(), &projection.name);
                    table_functions.insert(
                        internal_name.clone(),
                        Arc::new(SourceModelProjectionTableFunction { schema })
                            as Arc<dyn TableFunctionImpl>,
                    );
                    table_function_infos.push(registered_table_function(
                        self.schema_name(),
                        projection,
                        operation,
                        internal_name,
                    ));
                }
            }
        }

        let secret_keys = self.source_secrets.keys().cloned().collect::<BTreeSet<_>>();
        let inputs = build_registered_inputs(
            &self.manifest.declared_inputs,
            &self.source_variables,
            &secret_keys,
        );

        Ok(BackendRegistration {
            tables,
            table_functions,
            source: RegisteredSource {
                schema_name: self.manifest.common.name.clone(),
                tables: table_infos,
                table_functions: table_function_infos,
                inputs,
            },
        })
    }
}

fn operations_by_ref(ir: &SourceModelIr) -> HashMap<(&str, &str), &SourceModelOperation> {
    ir.operations
        .iter()
        .map(|operation| {
            (
                (operation.surface.as_str(), operation.id.as_str()),
                operation,
            )
        })
        .collect()
}

fn resolve_projection_operation<'a>(
    source_schema: &str,
    projection: &SourceModelProjection,
    operations: &'a HashMap<(&str, &str), &SourceModelOperation>,
) -> Result<&'a SourceModelOperation> {
    operations
        .get(&(
            projection.operation.surface.as_str(),
            projection.operation.operation.as_str(),
        ))
        .copied()
        .ok_or_else(|| {
            DataFusionError::Plan(format!(
                "source schema '{source_schema}' projection '{}' references missing operation '{}.{}'",
                projection.name, projection.operation.surface, projection.operation.operation
            ))
        })
}

fn required_operation_inputs(operation: &SourceModelOperation) -> Vec<String> {
    operation
        .inputs
        .iter()
        .filter(|input| input.required)
        .map(|input| input.name.clone())
        .collect()
}

fn validate_table_projection_inputs(
    source_schema: &str,
    projection: &SourceModelProjection,
    operation: &SourceModelOperation,
    required_inputs: &[String],
) -> Result<()> {
    let projected_columns = projection
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    for input in required_inputs {
        if !projected_columns.contains(input.as_str()) {
            return Err(DataFusionError::Plan(format!(
                "source schema '{source_schema}' projection/table '{}' references operation '{}.{}' but is missing required operation input '{input}'",
                projection.name, operation.surface, operation.id
            )));
        }
    }
    Ok(())
}

fn registered_table(
    projection: &SourceModelProjection,
    required_inputs: &[String],
) -> RegisteredTable {
    RegisteredTable {
        table_name: projection.name.clone(),
        description: String::new(),
        guide: String::new(),
        columns: registered_columns_from_specs(&projection.columns, required_inputs),
        required_filters: required_inputs.to_vec(),
    }
}

fn registered_table_function(
    schema_name: &str,
    projection: &SourceModelProjection,
    operation: &SourceModelOperation,
    internal_name: String,
) -> RegisteredTableFunction {
    let arguments = operation
        .inputs
        .iter()
        .map(argument_json)
        .collect::<Vec<_>>();
    let result_columns = registered_columns_from_specs(&projection.columns, &[])
        .into_iter()
        .map(|column| column_json(&column))
        .collect::<Vec<_>>();

    RegisteredTableFunction {
        schema_name: schema_name.to_string(),
        function_name: projection.name.clone(),
        internal_name,
        description: String::new(),
        arguments_json: serde_json::to_string(&arguments).expect("arguments json"),
        result_columns_json: serde_json::to_string(&result_columns).expect("result columns json"),
        arg_names: operation
            .inputs
            .iter()
            .map(|input| input.name.clone())
            .collect(),
    }
}

fn argument_json(input: &OperationInput) -> serde_json::Value {
    serde_json::json!({
        "name": input.name,
        "required": input.required,
        "values": Vec::<String>::new(),
    })
}

fn column_json(column: &RegisteredColumn) -> serde_json::Value {
    serde_json::json!({
        "name": column.name,
        "type": column.data_type,
        "nullable": column.nullable,
        "description": column.description,
    })
}

#[derive(Debug)]
struct SourceModelProjectionTableFunction {
    schema: datafusion::arrow::datatypes::SchemaRef,
}

impl TableFunctionImpl for SourceModelProjectionTableFunction {
    fn call(&self, _args: &[Expr]) -> Result<Arc<dyn TableProvider>> {
        Ok(Arc::new(EmptyTable::new(self.schema.clone())))
    }
}
