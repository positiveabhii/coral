//! Runtime for DSL v4 source-model projections.
//!
//! This backend resolves explicit SQL projections against materialized
//! importer IR, converts REST operations into the existing HTTP backend shape,
//! and then delegates execution to the established HTTP machinery.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::error::{DataFusionError, Result};
use datafusion::prelude::SessionContext;

use crate::backends::http;
use crate::backends::{BackendCompileRequest, BackendRegistration, CompiledBackendSource};
use crate::{CoreError, RequestAuthenticator};
use coral_spec::backends::http::{HttpSourceManifest, HttpTableSpec};
use coral_spec::{
    BodySpec, FilterMode, FilterSpec, FunctionArgBinding, HeaderSpec, OperationDetails,
    OperationInput, OperationResult, PaginationMode, PaginationSpec, ParsedTemplate,
    ProjectionKind, QueryParamSpec, RequestSpec, ResponseSpec, RestOperationDetails,
    RestPagination, RestParameterLocation, SourceManifestCommon, SourceModelIr,
    SourceModelManifestSurface, SourceModelOperation, SourceModelProjection,
    SourceModelSourceManifest, SourceTableFunctionSpec, TableCommon, TableFunctionArgSpec,
    ValueSourceSpec,
};

#[derive(Debug, Clone)]
struct SourceModelCompiledSource {
    manifest: SourceModelSourceManifest,
    ir: SourceModelIr,
    source_secrets: BTreeMap<String, String>,
    source_variables: BTreeMap<String, String>,
    request_authenticators: HashMap<String, Arc<dyn RequestAuthenticator>>,
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
        request_authenticators: request.request_authenticators.clone(),
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

    async fn register(&self, ctx: &SessionContext) -> Result<BackendRegistration> {
        let operations = operations_by_ref(&self.ir);
        let surface = selected_runtime_surface(self.schema_name(), &self.manifest)?;
        let mut tables = Vec::new();
        let mut functions = Vec::new();

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
                    tables.push(rest_projection_table(
                        self.schema_name(),
                        projection,
                        operation,
                    )?);
                }
                ProjectionKind::Function => {
                    functions.push(rest_projection_function(
                        self.schema_name(),
                        projection,
                        operation,
                    )?);
                }
            }
        }

        let http_manifest = HttpSourceManifest {
            common: SourceManifestCommon {
                dsl_version: self.manifest.common.dsl_version,
                name: self.manifest.common.name.clone(),
                version: self.manifest.common.version.clone(),
                description: self.manifest.common.description.clone(),
                test_queries: self.manifest.common.test_queries.clone(),
            },
            base_url: surface.base_url.clone(),
            auth: surface.auth.clone(),
            request_headers: surface.request_headers.clone(),
            rate_limit: surface.rate_limit.clone(),
            tables,
            functions,
            declared_inputs: self.manifest.declared_inputs.clone(),
        };

        http::compile_source(
            http_manifest,
            self.source_secrets.clone(),
            self.source_variables.clone(),
            self.request_authenticators.clone(),
        )
        .register(ctx)
        .await
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

fn selected_runtime_surface<'a>(
    source_schema: &str,
    manifest: &'a SourceModelSourceManifest,
) -> Result<&'a SourceModelManifestSurface> {
    let Some(first_projection) = manifest.projections.first() else {
        return Err(DataFusionError::Plan(format!(
            "source schema '{source_schema}' has no source-model projections"
        )));
    };
    let surface_id = first_projection.operation.surface.as_str();
    if let Some(other) = manifest
        .projections
        .iter()
        .find(|projection| projection.operation.surface != surface_id)
    {
        return Err(DataFusionError::Plan(format!(
            "source schema '{source_schema}' references multiple source-model surfaces ('{surface_id}' and '{}'); the first source-model REST runtime supports one surface",
            other.operation.surface
        )));
    }
    manifest
        .surfaces
        .iter()
        .find(|surface| surface.id == surface_id)
        .ok_or_else(|| {
            DataFusionError::Plan(format!(
                "source schema '{source_schema}' references unknown source-model surface '{surface_id}'"
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

fn rest_projection_table(
    source_schema: &str,
    projection: &SourceModelProjection,
    operation: &SourceModelOperation,
) -> Result<HttpTableSpec> {
    let rest = rest_details(source_schema, projection, operation)?;
    Ok(HttpTableSpec {
        common: TableCommon {
            name: projection.name.clone(),
            description: operation.description.clone(),
            guide: String::new(),
            filters: operation_inputs_as_filters(&operation.inputs),
            fetch_limit_default: None,
            columns: projection.columns.clone(),
        },
        request: rest_request(
            source_schema,
            &projection.name,
            rest,
            RequestBinding::Filter,
        )?,
        requests: Vec::new(),
        response: rest_response(&operation.result, rest),
        pagination: rest_pagination(rest),
    })
}

fn rest_projection_function(
    source_schema: &str,
    projection: &SourceModelProjection,
    operation: &SourceModelOperation,
) -> Result<SourceTableFunctionSpec> {
    let rest = rest_details(source_schema, projection, operation)?;
    Ok(SourceTableFunctionSpec {
        name: projection.name.clone(),
        description: operation.description.clone(),
        fetch_limit_default: None,
        args: operation_inputs_as_args(&operation.inputs),
        request: rest_request(source_schema, &projection.name, rest, RequestBinding::Arg)?,
        response: rest_response(&operation.result, rest),
        pagination: rest_pagination(rest),
        columns: projection.columns.clone(),
    })
}

fn rest_details<'a>(
    source_schema: &str,
    projection: &SourceModelProjection,
    operation: &'a SourceModelOperation,
) -> Result<&'a RestOperationDetails> {
    match &operation.details {
        OperationDetails::Rest { rest } => Ok(rest),
        OperationDetails::Graphql { .. } => Err(DataFusionError::Plan(format!(
            "source schema '{source_schema}' projection '{}' references operation '{}.{}', but only REST operations are executable in this source-model runtime",
            projection.name, operation.surface, operation.id
        ))),
    }
}

fn operation_inputs_as_filters(inputs: &[OperationInput]) -> Vec<FilterSpec> {
    inputs
        .iter()
        .map(|input| FilterSpec {
            name: input.name.clone(),
            required: input.required,
            mode: FilterMode::default(),
        })
        .collect()
}

fn operation_inputs_as_args(inputs: &[OperationInput]) -> Vec<TableFunctionArgSpec> {
    inputs
        .iter()
        .map(|input| TableFunctionArgSpec {
            name: input.name.clone(),
            required: input.required,
            values: Vec::new(),
            bind: FunctionArgBinding {
                arg: input.name.clone(),
            },
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum RequestBinding {
    Filter,
    Arg,
}

fn rest_request(
    source_schema: &str,
    projection_name: &str,
    rest: &RestOperationDetails,
    binding: RequestBinding,
) -> Result<RequestSpec> {
    if rest.request_body.is_some() {
        return Err(DataFusionError::Plan(format!(
            "source schema '{source_schema}' projection '{projection_name}' references REST operation with a request body; source-model REST execution currently supports path, query, and header parameters"
        )));
    }

    let mut path = rest.path.clone();
    let mut query = Vec::new();
    let mut headers = Vec::new();
    for parameter in &rest.parameters {
        match parameter.location {
            RestParameterLocation::Path => {
                let value = template_token(binding, &parameter.input);
                path = replace_path_parameter(&path, &parameter.name, &value);
            }
            RestParameterLocation::Query => {
                query.push(QueryParamSpec {
                    name: parameter.name.clone(),
                    value: bound_value(binding, &parameter.input),
                });
            }
            RestParameterLocation::Header => {
                headers.push(HeaderSpec {
                    name: parameter.name.clone(),
                    value: bound_value(binding, &parameter.input),
                });
            }
            RestParameterLocation::Cookie => {
                return Err(DataFusionError::Plan(format!(
                    "source schema '{source_schema}' projection '{projection_name}' references REST cookie parameter '{}', which is not supported by source-model REST execution",
                    parameter.name
                )));
            }
        }
    }

    Ok(RequestSpec {
        method: rest.method,
        path: ParsedTemplate::parse(path).map_err(|error| {
            DataFusionError::Plan(format!(
                "source schema '{source_schema}' projection '{projection_name}' has invalid REST path template: {error}"
            ))
        })?,
        query,
        body: BodySpec::default(),
        headers,
    })
}

fn replace_path_parameter(path: &str, parameter: &str, value: &str) -> String {
    path.replace(&format!("{{{parameter}}}"), value)
        .replace(&format!("{{+{parameter}}}"), value)
}

fn template_token(binding: RequestBinding, input: &str) -> String {
    match binding {
        RequestBinding::Filter => format!("{{{{filter.{input}}}}}"),
        RequestBinding::Arg => format!("{{{{arg.{input}}}}}"),
    }
}

fn bound_value(binding: RequestBinding, input: &str) -> ValueSourceSpec {
    match binding {
        RequestBinding::Filter => ValueSourceSpec::Filter {
            key: input.to_string(),
            default: None,
        },
        RequestBinding::Arg => ValueSourceSpec::Arg {
            key: input.to_string(),
            default: None,
        },
    }
}

fn rest_response(result: &OperationResult, rest: &RestOperationDetails) -> ResponseSpec {
    let rows_path = match result {
        OperationResult::Unit => Vec::new(),
        OperationResult::Single { .. } | OperationResult::List { .. } => rest
            .responses
            .iter()
            .find(|response| !response.error)
            .map(|response| response.body_path.clone())
            .unwrap_or_default(),
        OperationResult::WrappedList { items_path, .. } => items_path.clone(),
    };

    ResponseSpec {
        rows_path,
        ..ResponseSpec::default()
    }
}

fn rest_pagination(rest: &RestOperationDetails) -> PaginationSpec {
    match &rest.pagination {
        Some(RestPagination::LinkHeader { .. }) => PaginationSpec {
            mode: PaginationMode::LinkHeader,
            ..PaginationSpec::default()
        },
        Some(RestPagination::Page {
            page_input,
            page_size_input: _,
        }) => PaginationSpec {
            mode: PaginationMode::Page,
            page_param: Some(page_input.clone()),
            page_start: 1,
            ..PaginationSpec::default()
        },
        None => PaginationSpec::default(),
    }
}
