#![allow(
    missing_docs,
    reason = "This module defines field-heavy internal source-model IR types."
)]
#![allow(
    clippy::module_name_repetitions,
    reason = "The source-model prefix keeps public IR types unambiguous at crate boundaries."
)]
#![allow(
    clippy::too_many_lines,
    reason = "Validation and representative round-trip fixtures are clearer when kept with the IR."
)]

//! Source-model intermediate representation.
//!
//! The source-model IR is importer output, not an author-facing manifest
//! surface. It models provider API surfaces, their imported operations, type
//! shapes, and surface-scoped entity candidates. SQL projections live outside
//! this core IR, but validation accepts projection references so callers can
//! prove that authored projections resolve to imported operations.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{HttpMethod, ManifestError, Result, SourceModelProjectionRef};

pub const SOURCE_MODEL_IR_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceModelIr {
    pub ir_version: u32,
    pub surfaces: Vec<SourceModelSurface>,
    #[serde(default)]
    pub types: Vec<TypeDefinition>,
    #[serde(default)]
    pub operations: Vec<SourceModelOperation>,
    #[serde(default)]
    pub entities: Vec<EntityCandidate>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceModelSurface {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub protocol: SurfaceProtocol,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProtocol {
    Rest,
    Graphql,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TypeDefinition {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub kind: TypeDefinitionKind,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeDefinitionKind {
    Scalar {
        scalar: ScalarType,
    },
    Enum {
        values: Vec<EnumValue>,
    },
    Object {
        fields: Vec<ObjectField>,
    },
    InputObject {
        fields: Vec<InputField>,
    },
    Interface {
        fields: Vec<ObjectField>,
        #[serde(default)]
        implementations: Vec<String>,
    },
    Union {
        members: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    String,
    Integer,
    Float,
    Boolean,
    Id,
    Timestamp,
    Uri,
    Json,
    Null,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnumValue {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectField {
    pub name: String,
    pub ty: TypeRef,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<OperationInput>,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputField {
    pub name: String,
    pub ty: TypeRef,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OperationInput {
    pub name: String,
    pub ty: TypeRef,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TypeRef {
    #[serde(default)]
    pub nullable: bool,
    #[serde(flatten)]
    pub kind: TypeRefKind,
}

impl TypeRef {
    pub fn named(type_id: impl Into<String>) -> Self {
        Self {
            nullable: false,
            kind: TypeRefKind::Named {
                type_id: type_id.into(),
            },
        }
    }

    pub fn scalar(scalar: ScalarType) -> Self {
        Self {
            nullable: false,
            kind: TypeRefKind::Scalar { scalar },
        }
    }

    pub fn list(item: TypeRef) -> Self {
        Self {
            nullable: false,
            kind: TypeRefKind::List {
                item: Box::new(item),
            },
        }
    }

    pub fn map(value: TypeRef) -> Self {
        Self {
            nullable: false,
            kind: TypeRefKind::Map {
                value: Box::new(value),
            },
        }
    }

    pub fn unit() -> Self {
        Self {
            nullable: false,
            kind: TypeRefKind::Unit,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeRefKind {
    Named { type_id: String },
    Scalar { scalar: ScalarType },
    List { item: Box<TypeRef> },
    Map { value: Box<TypeRef> },
    Any,
    Unit,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceModelOperation {
    pub id: String,
    pub surface: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<OperationInput>,
    pub result: OperationResult,
    #[serde(flatten)]
    pub details: OperationDetails,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum OperationDetails {
    Rest {
        rest: Box<RestOperationDetails>,
    },
    Graphql {
        graphql: Box<GraphqlOperationDetails>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "cardinality", rename_all = "snake_case")]
pub enum OperationResult {
    Unit,
    Single {
        ty: TypeRef,
    },
    List {
        item: TypeRef,
    },
    WrappedList {
        item: TypeRef,
        items_path: Vec<String>,
        #[serde(default)]
        total_count_path: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestOperationDetails {
    pub method: HttpMethod,
    pub path: String,
    #[serde(default)]
    pub parameters: Vec<RestParameter>,
    #[serde(default)]
    pub request_body: Option<RestRequestBody>,
    #[serde(default)]
    pub responses: Vec<RestResponse>,
    #[serde(default)]
    pub pagination: Option<RestPagination>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestParameter {
    pub name: String,
    pub input: String,
    pub location: RestParameterLocation,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub explode: Option<bool>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestRequestBody {
    pub input: String,
    pub content_type: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RestResponse {
    pub status: RestStatusCode,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub body: Option<TypeRef>,
    #[serde(default)]
    pub body_path: Vec<String>,
    #[serde(default)]
    pub error: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RestStatusCode {
    Code(u16),
    Range(String),
    Default(String),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RestPagination {
    LinkHeader {
        #[serde(default)]
        next_rel: Option<String>,
        #[serde(default)]
        page_size_input: Option<String>,
    },
    Page {
        page_input: String,
        #[serde(default)]
        page_size_input: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphqlOperationDetails {
    pub operation: GraphqlOperationKind,
    pub root_field_path: Vec<String>,
    #[serde(default)]
    pub variables: Vec<GraphqlVariable>,
    #[serde(default)]
    pub selection: GraphqlSelectionPolicy,
    #[serde(default)]
    pub response_data_path: Vec<String>,
    #[serde(default)]
    pub pagination: Option<GraphqlPagination>,
    #[serde(default)]
    pub partial_data_policy: GraphqlPartialDataPolicy,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlOperationKind {
    Query,
    Mutation,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphqlVariable {
    pub name: String,
    pub input: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum GraphqlSelectionPolicy {
    #[default]
    ProjectionColumns,
    Explicit {
        fields: Vec<FieldPath>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphqlPagination {
    #[serde(default)]
    pub connection_path: Vec<String>,
    #[serde(default)]
    pub cursor_input: Option<String>,
    #[serde(default)]
    pub page_size_input: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraphqlPartialDataPolicy {
    #[default]
    Reject,
    AcceptWithErrors,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EntityCandidate {
    pub surface: String,
    pub entity: String,
    #[serde(default)]
    pub keys: Vec<IdentityKey>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityKey {
    pub name: String,
    pub fields: Vec<FieldPath>,
}

pub type FieldPath = Vec<String>;

impl SourceModelIr {
    pub fn validate(&self, projections: &[SourceModelProjectionRef]) -> Result<()> {
        if self.ir_version != SOURCE_MODEL_IR_VERSION {
            return Err(ManifestError::validation(format!(
                "source model IR version {} is not supported; expected {SOURCE_MODEL_IR_VERSION}",
                self.ir_version
            )));
        }

        let surfaces = validate_surfaces(&self.surfaces)?;
        let types = validate_types(&self.types)?;
        let operations = validate_operations(&self.operations, &surfaces, &types)?;
        validate_entities(&self.entities, &surfaces, &types)?;
        validate_projection_refs(projections, &operations)
    }
}

fn validate_surfaces(
    surfaces: &[SourceModelSurface],
) -> Result<HashMap<&str, &SourceModelSurface>> {
    let mut seen = HashMap::new();
    if surfaces.is_empty() {
        return Err(ManifestError::validation(
            "source model IR must define at least one surface",
        ));
    }

    for surface in surfaces {
        validate_id(&surface.id, "surface id")?;
        if seen.insert(surface.id.as_str(), surface).is_some() {
            return Err(ManifestError::validation(format!(
                "source model IR declares surface '{}' more than once",
                surface.id
            )));
        }
    }

    Ok(seen)
}

fn validate_types(types: &[TypeDefinition]) -> Result<HashSet<&str>> {
    let mut seen = HashSet::new();

    for ty in types {
        validate_id(&ty.id, "type id")?;
        if !seen.insert(ty.id.as_str()) {
            return Err(ManifestError::validation(format!(
                "source model IR declares type '{}' more than once",
                ty.id
            )));
        }
    }

    for ty in types {
        match &ty.kind {
            TypeDefinitionKind::Scalar { .. } => {}
            TypeDefinitionKind::Enum { values } => validate_enum_values(&ty.id, values)?,
            TypeDefinitionKind::Object { fields } => validate_object_fields(&ty.id, fields, &seen)?,
            TypeDefinitionKind::InputObject { fields } => {
                validate_input_fields(&ty.id, fields, &seen)?;
            }
            TypeDefinitionKind::Interface {
                fields,
                implementations,
            } => {
                validate_object_fields(&ty.id, fields, &seen)?;
                for implementation in implementations {
                    validate_named_type_ref(&ty.id, implementation, &seen, "implementation")?;
                }
            }
            TypeDefinitionKind::Union { members } => {
                if members.is_empty() {
                    return Err(ManifestError::validation(format!(
                        "source model type '{}' union must have at least one member",
                        ty.id
                    )));
                }
                validate_unique_strings(
                    members,
                    &format!("source model type '{}' union member", ty.id),
                )?;
                for member in members {
                    validate_named_type_ref(&ty.id, member, &seen, "union member")?;
                }
            }
        }
    }

    Ok(seen)
}

fn validate_enum_values(type_id: &str, values: &[EnumValue]) -> Result<()> {
    if values.is_empty() {
        return Err(ManifestError::validation(format!(
            "source model type '{type_id}' enum must have at least one value"
        )));
    }
    let mut seen = HashSet::new();
    for value in values {
        validate_id(
            &value.name,
            &format!("source model type '{type_id}' enum value"),
        )?;
        if !seen.insert(value.name.as_str()) {
            return Err(ManifestError::validation(format!(
                "source model type '{type_id}' enum value '{}' is declared more than once",
                value.name
            )));
        }
    }
    Ok(())
}

fn validate_object_fields(
    type_id: &str,
    fields: &[ObjectField],
    types: &HashSet<&str>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for field in fields {
        validate_id(&field.name, &format!("source model type '{type_id}' field"))?;
        if !seen.insert(field.name.as_str()) {
            return Err(ManifestError::validation(format!(
                "source model type '{type_id}' field '{}' is declared more than once",
                field.name
            )));
        }
        validate_type_ref(&field.ty, types, &format!("source model type '{type_id}'"))?;
        validate_operation_inputs(
            &field.arguments,
            types,
            &format!("source model type '{type_id}' field '{}'", field.name),
        )?;
    }
    Ok(())
}

fn validate_input_fields(
    type_id: &str,
    fields: &[InputField],
    types: &HashSet<&str>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for field in fields {
        validate_id(
            &field.name,
            &format!("source model type '{type_id}' input field"),
        )?;
        if !seen.insert(field.name.as_str()) {
            return Err(ManifestError::validation(format!(
                "source model type '{type_id}' input field '{}' is declared more than once",
                field.name
            )));
        }
        validate_type_ref(&field.ty, types, &format!("source model type '{type_id}'"))?;
    }
    Ok(())
}

fn validate_operations<'a>(
    operations: &'a [SourceModelOperation],
    surfaces: &HashMap<&str, &SourceModelSurface>,
    types: &HashSet<&str>,
) -> Result<HashSet<(&'a str, &'a str)>> {
    let mut seen = HashSet::new();

    for operation in operations {
        validate_id(&operation.id, "operation id")?;
        let Some(surface) = surfaces.get(operation.surface.as_str()) else {
            return Err(ManifestError::validation(format!(
                "source model operation '{}' references unknown surface '{}'",
                operation.id, operation.surface
            )));
        };
        if !seen.insert((operation.surface.as_str(), operation.id.as_str())) {
            return Err(ManifestError::validation(format!(
                "source model surface '{}' declares operation '{}' more than once",
                operation.surface, operation.id
            )));
        }

        validate_operation_inputs(
            &operation.inputs,
            types,
            &format!("source model operation '{}'", operation.id),
        )?;
        validate_operation_result(
            &operation.result,
            types,
            &format!("source model operation '{}'", operation.id),
        )?;
        validate_operation_details(operation, surface, types)?;
    }

    Ok(seen)
}

fn validate_operation_inputs(
    inputs: &[OperationInput],
    types: &HashSet<&str>,
    context: &str,
) -> Result<HashSet<String>> {
    let mut seen = HashSet::new();
    for input in inputs {
        validate_id(&input.name, &format!("{context} input"))?;
        if !seen.insert(input.name.clone()) {
            return Err(ManifestError::validation(format!(
                "{context} input '{}' is declared more than once",
                input.name
            )));
        }
        validate_type_ref(&input.ty, types, context)?;
    }
    Ok(seen)
}

fn validate_operation_result(
    result: &OperationResult,
    types: &HashSet<&str>,
    context: &str,
) -> Result<()> {
    match result {
        OperationResult::Unit => Ok(()),
        OperationResult::Single { ty } => validate_type_ref(ty, types, context),
        OperationResult::List { item } => validate_type_ref(item, types, context),
        OperationResult::WrappedList {
            item,
            items_path,
            total_count_path,
        } => {
            validate_type_ref(item, types, context)?;
            validate_field_path(items_path, &format!("{context} wrapped list items_path"))?;
            if !total_count_path.is_empty() {
                validate_field_path(
                    total_count_path,
                    &format!("{context} wrapped list total_count_path"),
                )?;
            }
            Ok(())
        }
    }
}

fn validate_operation_details(
    operation: &SourceModelOperation,
    surface: &SourceModelSurface,
    types: &HashSet<&str>,
) -> Result<()> {
    let input_names = operation
        .inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect::<HashSet<_>>();

    match &operation.details {
        OperationDetails::Rest { rest } => {
            if surface.protocol != SurfaceProtocol::Rest {
                return Err(ManifestError::validation(format!(
                    "source model operation '{}' has REST details but surface '{}' is {:?}",
                    operation.id, operation.surface, surface.protocol
                )));
            }
            validate_rest_operation(operation, rest, &input_names, types)
        }
        OperationDetails::Graphql { graphql } => {
            if surface.protocol != SurfaceProtocol::Graphql {
                return Err(ManifestError::validation(format!(
                    "source model operation '{}' has GraphQL details but surface '{}' is {:?}",
                    operation.id, operation.surface, surface.protocol
                )));
            }
            validate_graphql_operation(operation, graphql, &input_names)
        }
    }
}

fn validate_rest_operation(
    operation: &SourceModelOperation,
    rest: &RestOperationDetails,
    input_names: &HashSet<&str>,
    types: &HashSet<&str>,
) -> Result<()> {
    if rest.path.trim().is_empty() {
        return Err(ManifestError::validation(format!(
            "source model operation '{}' has an empty REST path",
            operation.id
        )));
    }

    for parameter in &rest.parameters {
        validate_id(
            &parameter.name,
            &format!("source model operation '{}' REST parameter", operation.id),
        )?;
        validate_input_ref(operation, &parameter.input, input_names, "REST parameter")?;
    }

    if let Some(request_body) = &rest.request_body {
        if request_body.content_type.trim().is_empty() {
            return Err(ManifestError::validation(format!(
                "source model operation '{}' REST request body has an empty content_type",
                operation.id
            )));
        }
        validate_input_ref(
            operation,
            &request_body.input,
            input_names,
            "REST request body",
        )?;
    }

    for response in &rest.responses {
        if let Some(body) = &response.body {
            validate_type_ref(
                body,
                types,
                &format!("source model operation '{}' REST response", operation.id),
            )?;
        }
        if !response.body_path.is_empty() {
            validate_field_path(
                &response.body_path,
                &format!(
                    "source model operation '{}' REST response body_path",
                    operation.id
                ),
            )?;
        }
    }

    if let Some(pagination) = &rest.pagination {
        validate_rest_pagination(operation, pagination, input_names)?;
    }

    Ok(())
}

fn validate_rest_pagination(
    operation: &SourceModelOperation,
    pagination: &RestPagination,
    input_names: &HashSet<&str>,
) -> Result<()> {
    match pagination {
        RestPagination::LinkHeader {
            page_size_input, ..
        } => {
            if let Some(input) = page_size_input {
                validate_input_ref(operation, input, input_names, "REST pagination page_size")?;
            }
        }
        RestPagination::Page {
            page_input,
            page_size_input,
        } => {
            validate_input_ref(operation, page_input, input_names, "REST pagination page")?;
            if let Some(input) = page_size_input {
                validate_input_ref(operation, input, input_names, "REST pagination page_size")?;
            }
        }
    }
    Ok(())
}

fn validate_graphql_operation(
    operation: &SourceModelOperation,
    graphql: &GraphqlOperationDetails,
    input_names: &HashSet<&str>,
) -> Result<()> {
    validate_field_path(
        &graphql.root_field_path,
        &format!(
            "source model operation '{}' GraphQL root_field_path",
            operation.id
        ),
    )?;

    for variable in &graphql.variables {
        validate_id(
            &variable.name,
            &format!("source model operation '{}' GraphQL variable", operation.id),
        )?;
        validate_input_ref(operation, &variable.input, input_names, "GraphQL variable")?;
    }

    if !graphql.response_data_path.is_empty() {
        validate_field_path(
            &graphql.response_data_path,
            &format!(
                "source model operation '{}' GraphQL response_data_path",
                operation.id
            ),
        )?;
    }

    match &graphql.selection {
        GraphqlSelectionPolicy::ProjectionColumns => {}
        GraphqlSelectionPolicy::Explicit { fields } => {
            for field in fields {
                validate_field_path(
                    field,
                    &format!(
                        "source model operation '{}' GraphQL selection",
                        operation.id
                    ),
                )?;
            }
        }
    }

    if let Some(pagination) = &graphql.pagination {
        if !pagination.connection_path.is_empty() {
            validate_field_path(
                &pagination.connection_path,
                &format!(
                    "source model operation '{}' GraphQL pagination connection_path",
                    operation.id
                ),
            )?;
        }
        if let Some(input) = &pagination.cursor_input {
            validate_input_ref(operation, input, input_names, "GraphQL pagination cursor")?;
        }
        if let Some(input) = &pagination.page_size_input {
            validate_input_ref(
                operation,
                input,
                input_names,
                "GraphQL pagination page_size",
            )?;
        }
    }

    Ok(())
}

fn validate_entities(
    entities: &[EntityCandidate],
    surfaces: &HashMap<&str, &SourceModelSurface>,
    types: &HashSet<&str>,
) -> Result<()> {
    let mut seen = HashSet::new();

    for entity in entities {
        if !surfaces.contains_key(entity.surface.as_str()) {
            return Err(ManifestError::validation(format!(
                "source model entity '{}' references unknown surface '{}'",
                entity.entity, entity.surface
            )));
        }
        validate_named_type_ref(&entity.entity, &entity.entity, types, "entity")?;
        if !seen.insert((entity.surface.as_str(), entity.entity.as_str())) {
            return Err(ManifestError::validation(format!(
                "source model surface '{}' declares entity '{}' more than once",
                entity.surface, entity.entity
            )));
        }
        for key in &entity.keys {
            validate_id(
                &key.name,
                &format!("source model entity '{}' identity key", entity.entity),
            )?;
            if key.fields.is_empty() {
                return Err(ManifestError::validation(format!(
                    "source model entity '{}' identity key '{}' must contain at least one field path",
                    entity.entity, key.name
                )));
            }
            for field in &key.fields {
                validate_field_path(
                    field,
                    &format!(
                        "source model entity '{}' identity key '{}'",
                        entity.entity, key.name
                    ),
                )?;
            }
        }
    }

    Ok(())
}

fn validate_projection_refs(
    projections: &[SourceModelProjectionRef],
    operations: &HashSet<(&str, &str)>,
) -> Result<()> {
    let mut seen = HashSet::new();

    for projection in projections {
        validate_id(&projection.name, "source model projection name")?;
        if !seen.insert(projection.name.as_str()) {
            return Err(ManifestError::validation(format!(
                "source model projection '{}' is declared more than once",
                projection.name
            )));
        }
        if !operations.contains(&(
            projection.operation.surface.as_str(),
            projection.operation.operation.as_str(),
        )) {
            return Err(ManifestError::validation(format!(
                "source model projection '{}' references unknown operation '{}.{}'",
                projection.name, projection.operation.surface, projection.operation.operation
            )));
        }
    }

    Ok(())
}

fn validate_type_ref(ty: &TypeRef, types: &HashSet<&str>, context: &str) -> Result<()> {
    match &ty.kind {
        TypeRefKind::Named { type_id } => validate_named_type_ref(context, type_id, types, "type"),
        TypeRefKind::Scalar { .. } | TypeRefKind::Any | TypeRefKind::Unit => Ok(()),
        TypeRefKind::List { item } => validate_type_ref(item, types, context),
        TypeRefKind::Map { value } => validate_type_ref(value, types, context),
    }
}

fn validate_named_type_ref(
    context: &str,
    type_id: &str,
    types: &HashSet<&str>,
    ref_kind: &str,
) -> Result<()> {
    validate_id(type_id, &format!("{context} {ref_kind} reference"))?;
    if !types.contains(type_id) {
        return Err(ManifestError::validation(format!(
            "{context} references unknown {ref_kind} '{type_id}'"
        )));
    }
    Ok(())
}

fn validate_input_ref(
    operation: &SourceModelOperation,
    input: &str,
    input_names: &HashSet<&str>,
    context: &str,
) -> Result<()> {
    validate_id(
        input,
        &format!("source model operation '{}' {context} input", operation.id),
    )?;
    if !input_names.contains(input) {
        return Err(ManifestError::validation(format!(
            "source model operation '{}' {context} references unknown input '{input}'",
            operation.id
        )));
    }
    Ok(())
}

fn validate_field_path(path: &[String], context: &str) -> Result<()> {
    if path.is_empty() {
        return Err(ManifestError::validation(format!(
            "{context} must contain at least one segment"
        )));
    }

    for segment in path {
        validate_id(segment, context)?;
    }
    Ok(())
}

fn validate_unique_strings(values: &[String], context: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for value in values {
        validate_id(value, context)?;
        if !seen.insert(value.as_str()) {
            return Err(ManifestError::validation(format!(
                "{context} '{value}' is declared more than once"
            )));
        }
    }
    Ok(())
}

fn validate_id(value: &str, context: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ManifestError::validation(format!(
            "{context} must not be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        EntityCandidate, GraphqlOperationDetails, GraphqlOperationKind, GraphqlPagination,
        GraphqlPartialDataPolicy, GraphqlSelectionPolicy, IdentityKey, ObjectField,
        OperationDetails, OperationInput, OperationResult, RestOperationDetails, RestPagination,
        RestParameter, RestParameterLocation, RestResponse, RestStatusCode,
        SOURCE_MODEL_IR_VERSION, ScalarType, SourceModelIr, SourceModelOperation,
        SourceModelSurface, SurfaceProtocol, TypeDefinition, TypeDefinitionKind, TypeRef,
    };
    use crate::{HttpMethod, ProjectionKind, SourceModelOperationRef, SourceModelProjectionRef};

    #[test]
    fn source_model_ir_round_trips_representative_rest_fragment() {
        let model = rest_model();
        let projections = vec![SourceModelProjectionRef {
            name: "issues".to_string(),
            kind: ProjectionKind::Table,
            operation: SourceModelOperationRef {
                surface: "github-rest".to_string(),
                operation: "issues/list-for-repo".to_string(),
            },
        }];

        model
            .validate(&projections)
            .expect("representative REST model should validate");
        let yaml = serde_yaml::to_string(&model).expect("REST model should serialize");
        let decoded: SourceModelIr =
            serde_yaml::from_str(&yaml).expect("REST model should deserialize");

        assert_eq!(decoded, model);
        decoded
            .validate(&projections)
            .expect("round-tripped REST model should validate");
    }

    #[test]
    fn source_model_ir_round_trips_synthetic_graphql_fragment() {
        let model = graphql_model();
        let projections = vec![SourceModelProjectionRef {
            name: "repository_issues".to_string(),
            kind: ProjectionKind::Function,
            operation: SourceModelOperationRef {
                surface: "github-graphql".to_string(),
                operation: "repository.issues".to_string(),
            },
        }];

        model
            .validate(&projections)
            .expect("synthetic GraphQL model should validate");
        let yaml = serde_yaml::to_string(&model).expect("GraphQL model should serialize");
        let decoded: SourceModelIr =
            serde_yaml::from_str(&yaml).expect("GraphQL model should deserialize");

        assert_eq!(decoded, model);
        decoded
            .validate(&projections)
            .expect("round-tripped GraphQL model should validate");
    }

    #[test]
    fn source_model_ir_rejects_duplicate_surface_scoped_operation_ids() {
        let mut model = rest_model();
        let duplicate = model
            .operations
            .first()
            .expect("REST fixture has an operation")
            .clone();
        model.operations.push(duplicate);

        let error = model
            .validate(&[])
            .expect_err("duplicate operation IDs should fail");

        assert_eq!(
            error.to_string(),
            "source model surface 'github-rest' declares operation 'issues/list-for-repo' more than once"
        );
    }

    #[test]
    fn source_model_ir_rejects_invalid_projection_reference() {
        let model = rest_model();
        let projections = vec![SourceModelProjectionRef {
            name: "missing".to_string(),
            kind: ProjectionKind::Table,
            operation: SourceModelOperationRef {
                surface: "github-rest".to_string(),
                operation: "missing/op".to_string(),
            },
        }];

        let error = model
            .validate(&projections)
            .expect_err("missing projection operation should fail");

        assert_eq!(
            error.to_string(),
            "source model projection 'missing' references unknown operation 'github-rest.missing/op'"
        );
    }

    #[test]
    fn entity_candidates_are_surface_scoped() {
        let model = SourceModelIr {
            ir_version: SOURCE_MODEL_IR_VERSION,
            surfaces: vec![
                SourceModelSurface {
                    id: "rest".to_string(),
                    description: String::new(),
                    protocol: SurfaceProtocol::Rest,
                    base_url: Some("https://api.example.com".to_string()),
                },
                SourceModelSurface {
                    id: "graphql".to_string(),
                    description: String::new(),
                    protocol: SurfaceProtocol::Graphql,
                    base_url: Some("https://api.example.com/graphql".to_string()),
                },
            ],
            types: vec![issue_type()],
            operations: vec![],
            entities: vec![
                EntityCandidate {
                    surface: "rest".to_string(),
                    entity: "github.issue".to_string(),
                    keys: vec![IdentityKey {
                        name: "rest_path".to_string(),
                        fields: vec![
                            vec!["repository".to_string(), "owner".to_string()],
                            vec!["number".to_string()],
                        ],
                    }],
                },
                EntityCandidate {
                    surface: "graphql".to_string(),
                    entity: "github.issue".to_string(),
                    keys: vec![IdentityKey {
                        name: "node_id".to_string(),
                        fields: vec![vec!["id".to_string()]],
                    }],
                },
            ],
        };

        model
            .validate(&[])
            .expect("same entity type can be imported as separate surface-scoped candidates");
    }

    fn rest_model() -> SourceModelIr {
        SourceModelIr {
            ir_version: SOURCE_MODEL_IR_VERSION,
            surfaces: vec![SourceModelSurface {
                id: "github-rest".to_string(),
                description: "GitHub REST API".to_string(),
                protocol: SurfaceProtocol::Rest,
                base_url: Some("https://api.github.com".to_string()),
            }],
            types: vec![issue_type()],
            operations: vec![SourceModelOperation {
                id: "issues/list-for-repo".to_string(),
                surface: "github-rest".to_string(),
                description: "List repository issues".to_string(),
                inputs: vec![
                    OperationInput {
                        name: "owner".to_string(),
                        ty: TypeRef::scalar(ScalarType::String),
                        required: true,
                        description: String::new(),
                    },
                    OperationInput {
                        name: "repo".to_string(),
                        ty: TypeRef::scalar(ScalarType::String),
                        required: true,
                        description: String::new(),
                    },
                    OperationInput {
                        name: "per_page".to_string(),
                        ty: TypeRef::scalar(ScalarType::Integer),
                        required: false,
                        description: String::new(),
                    },
                ],
                result: OperationResult::List {
                    item: TypeRef::named("github.issue"),
                },
                details: OperationDetails::Rest {
                    rest: Box::new(RestOperationDetails {
                        method: HttpMethod::GET,
                        path: "/repos/{owner}/{repo}/issues".to_string(),
                        parameters: vec![
                            RestParameter {
                                name: "owner".to_string(),
                                input: "owner".to_string(),
                                location: RestParameterLocation::Path,
                                required: true,
                                style: None,
                                explode: None,
                            },
                            RestParameter {
                                name: "repo".to_string(),
                                input: "repo".to_string(),
                                location: RestParameterLocation::Path,
                                required: true,
                                style: None,
                                explode: None,
                            },
                            RestParameter {
                                name: "per_page".to_string(),
                                input: "per_page".to_string(),
                                location: RestParameterLocation::Query,
                                required: false,
                                style: Some("form".to_string()),
                                explode: Some(true),
                            },
                        ],
                        request_body: None,
                        responses: vec![RestResponse {
                            status: RestStatusCode::Code(200),
                            content_type: Some("application/json".to_string()),
                            body: Some(TypeRef::list(TypeRef::named("github.issue"))),
                            body_path: vec![],
                            error: false,
                        }],
                        pagination: Some(RestPagination::LinkHeader {
                            next_rel: Some("next".to_string()),
                            page_size_input: Some("per_page".to_string()),
                        }),
                    }),
                },
            }],
            entities: vec![EntityCandidate {
                surface: "github-rest".to_string(),
                entity: "github.issue".to_string(),
                keys: vec![IdentityKey {
                    name: "repo_issue_number".to_string(),
                    fields: vec![
                        vec!["repository_url".to_string()],
                        vec!["number".to_string()],
                    ],
                }],
            }],
        }
    }

    fn graphql_model() -> SourceModelIr {
        SourceModelIr {
            ir_version: SOURCE_MODEL_IR_VERSION,
            surfaces: vec![SourceModelSurface {
                id: "github-graphql".to_string(),
                description: "GitHub GraphQL API".to_string(),
                protocol: SurfaceProtocol::Graphql,
                base_url: Some("https://api.github.com/graphql".to_string()),
            }],
            types: vec![issue_type(), repository_type(), issue_state_type()],
            operations: vec![SourceModelOperation {
                id: "repository.issues".to_string(),
                surface: "github-graphql".to_string(),
                description: "Repository issues connection".to_string(),
                inputs: vec![
                    OperationInput {
                        name: "owner".to_string(),
                        ty: TypeRef::scalar(ScalarType::String),
                        required: true,
                        description: String::new(),
                    },
                    OperationInput {
                        name: "name".to_string(),
                        ty: TypeRef::scalar(ScalarType::String),
                        required: true,
                        description: String::new(),
                    },
                    OperationInput {
                        name: "first".to_string(),
                        ty: TypeRef::scalar(ScalarType::Integer),
                        required: false,
                        description: String::new(),
                    },
                    OperationInput {
                        name: "after".to_string(),
                        ty: TypeRef {
                            nullable: true,
                            kind: super::TypeRefKind::Scalar {
                                scalar: ScalarType::String,
                            },
                        },
                        required: false,
                        description: String::new(),
                    },
                ],
                result: OperationResult::WrappedList {
                    item: TypeRef::named("github.issue"),
                    items_path: vec![
                        "repository".to_string(),
                        "issues".to_string(),
                        "nodes".to_string(),
                    ],
                    total_count_path: vec![
                        "repository".to_string(),
                        "issues".to_string(),
                        "totalCount".to_string(),
                    ],
                },
                details: OperationDetails::Graphql {
                    graphql: Box::new(GraphqlOperationDetails {
                        operation: GraphqlOperationKind::Query,
                        root_field_path: vec!["repository".to_string(), "issues".to_string()],
                        variables: vec![
                            super::GraphqlVariable {
                                name: "owner".to_string(),
                                input: "owner".to_string(),
                            },
                            super::GraphqlVariable {
                                name: "name".to_string(),
                                input: "name".to_string(),
                            },
                            super::GraphqlVariable {
                                name: "first".to_string(),
                                input: "first".to_string(),
                            },
                            super::GraphqlVariable {
                                name: "after".to_string(),
                                input: "after".to_string(),
                            },
                        ],
                        selection: GraphqlSelectionPolicy::Explicit {
                            fields: vec![
                                vec!["id".to_string()],
                                vec!["number".to_string()],
                                vec!["title".to_string()],
                            ],
                        },
                        response_data_path: vec!["data".to_string()],
                        pagination: Some(GraphqlPagination {
                            connection_path: vec!["repository".to_string(), "issues".to_string()],
                            cursor_input: Some("after".to_string()),
                            page_size_input: Some("first".to_string()),
                        }),
                        partial_data_policy: GraphqlPartialDataPolicy::Reject,
                    }),
                },
            }],
            entities: vec![EntityCandidate {
                surface: "github-graphql".to_string(),
                entity: "github.issue".to_string(),
                keys: vec![IdentityKey {
                    name: "node_id".to_string(),
                    fields: vec![vec!["id".to_string()]],
                }],
            }],
        }
    }

    fn issue_type() -> TypeDefinition {
        TypeDefinition {
            id: "github.issue".to_string(),
            name: "Issue".to_string(),
            description: String::new(),
            kind: TypeDefinitionKind::Object {
                fields: vec![
                    ObjectField {
                        name: "id".to_string(),
                        ty: TypeRef::scalar(ScalarType::Id),
                        description: String::new(),
                        arguments: vec![],
                        deprecated: false,
                    },
                    ObjectField {
                        name: "number".to_string(),
                        ty: TypeRef::scalar(ScalarType::Integer),
                        description: String::new(),
                        arguments: vec![],
                        deprecated: false,
                    },
                    ObjectField {
                        name: "title".to_string(),
                        ty: TypeRef::scalar(ScalarType::String),
                        description: String::new(),
                        arguments: vec![],
                        deprecated: false,
                    },
                ],
            },
        }
    }

    fn repository_type() -> TypeDefinition {
        TypeDefinition {
            id: "github.repository".to_string(),
            name: "Repository".to_string(),
            description: String::new(),
            kind: TypeDefinitionKind::Object {
                fields: vec![ObjectField {
                    name: "issues".to_string(),
                    ty: TypeRef::list(TypeRef::named("github.issue")),
                    description: String::new(),
                    arguments: vec![
                        OperationInput {
                            name: "first".to_string(),
                            ty: TypeRef::scalar(ScalarType::Integer),
                            required: false,
                            description: String::new(),
                        },
                        OperationInput {
                            name: "after".to_string(),
                            ty: TypeRef {
                                nullable: true,
                                kind: super::TypeRefKind::Scalar {
                                    scalar: ScalarType::String,
                                },
                            },
                            required: false,
                            description: String::new(),
                        },
                    ],
                    deprecated: false,
                }],
            },
        }
    }

    fn issue_state_type() -> TypeDefinition {
        TypeDefinition {
            id: "github.issue_state".to_string(),
            name: "IssueState".to_string(),
            description: String::new(),
            kind: TypeDefinitionKind::Enum {
                values: vec![
                    super::EnumValue {
                        name: "OPEN".to_string(),
                        description: String::new(),
                        deprecated: false,
                    },
                    super::EnumValue {
                        name: "CLOSED".to_string(),
                        description: String::new(),
                        deprecated: false,
                    },
                ],
            },
        }
    }
}
