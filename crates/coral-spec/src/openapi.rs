#![allow(
    missing_docs,
    reason = "This module defines importer-owned structs and OpenAPI-shaped parsing helpers."
)]
#![allow(
    clippy::module_name_repetitions,
    reason = "The OpenAPI prefix keeps importer types clear at crate boundaries."
)]
#![allow(
    clippy::too_many_lines,
    reason = "The narrow importer and its fixture tests are easiest to audit together."
)]

//! `OpenAPI` importer for DSL v4 source-model IR.
//!
//! This importer intentionally handles a narrow `OpenAPI` 3 REST slice. It
//! imports usable operations when it can and records diagnostics for
//! unsupported constructs instead of treating every unsupported shape as a hard
//! failure.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::backends::source_model::{SourceModelManifestSurface, SurfaceDescriptionType};
use crate::{
    EntityCandidate, EnumValue, HttpMethod, IdentityKey, ManifestError, ObjectField,
    OperationDetails, OperationInput, OperationResult, RestOperationDetails, RestPagination,
    RestParameter, RestParameterLocation, RestRequestBody, RestResponse, RestStatusCode, Result,
    SOURCE_MODEL_IR_VERSION, ScalarType, SourceModelIr, SourceModelOperation, SourceModelSurface,
    SurfaceProtocol, TypeDefinition, TypeDefinitionKind, TypeRef, TypeRefKind,
};

pub const OPENAPI_IMPORTER_VERSION: &str = "openapi-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiImportResult {
    pub ir: SourceModelIr,
    pub diagnostics: Vec<OpenApiImportDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiImportDiagnostic {
    pub location: String,
    pub message: String,
}

/// Imports one pinned `OpenAPI` surface descriptor and document into source-model
/// IR.
pub fn import_openapi_surface(
    surface: &SourceModelManifestSurface,
    document: &[u8],
) -> Result<OpenApiImportResult> {
    validate_pinned_surface(surface, document)?;

    let document_value: Value =
        serde_yaml::from_slice(document).map_err(ManifestError::parse_yaml)?;
    let document = OpenApiDocument::new(document_value)?;
    let importer = OpenApiImporter::new(surface, document);
    importer.import()
}

fn validate_pinned_surface(surface: &SourceModelManifestSurface, document: &[u8]) -> Result<()> {
    if surface.surface_type != SurfaceDescriptionType::OpenApi {
        return Err(ManifestError::validation(format!(
            "surface '{}' has unsupported OpenAPI importer type",
            surface.id
        )));
    }
    if surface.url.trim().is_empty() || surface.sha256.trim().is_empty() {
        return Err(ManifestError::validation(format!(
            "surface '{}' must provide both url and sha256 before OpenAPI import",
            surface.id
        )));
    }
    let actual = sha256_hex(document);
    if !actual.eq_ignore_ascii_case(surface.sha256.trim()) {
        return Err(ManifestError::validation(format!(
            "surface '{}' OpenAPI document sha256 mismatch: expected {}, got {actual}",
            surface.id, surface.sha256
        )));
    }
    Ok(())
}

fn sha256_hex(document: &[u8]) -> String {
    let digest = Sha256::digest(document);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to a string cannot fail");
    }
    out
}

struct OpenApiDocument {
    root: Value,
}

impl OpenApiDocument {
    fn new(root: Value) -> Result<Self> {
        if !root.is_object() {
            return Err(ManifestError::validation(
                "OpenAPI document root must be a mapping",
            ));
        }
        Ok(Self { root })
    }

    fn object_at<'a>(&'a self, path: &[&str]) -> Option<&'a Map<String, Value>> {
        let mut value = &self.root;
        for segment in path {
            value = value.get(*segment)?;
        }
        value.as_object()
    }

    fn resolve_ref<'a>(&'a self, ref_value: &str) -> Option<&'a Value> {
        let pointer = ref_value.strip_prefix('#')?;
        self.root.pointer(pointer)
    }

    fn resolve_ref_cloned(&self, ref_value: &str) -> Option<Value> {
        self.resolve_ref(ref_value).cloned()
    }
}

struct OpenApiImporter {
    surface_id: String,
    base_url: String,
    document: OpenApiDocument,
    diagnostics: Vec<OpenApiImportDiagnostic>,
    types: Vec<TypeDefinition>,
    type_ids: HashSet<String>,
    operations: Vec<SourceModelOperation>,
    operation_ids: HashMap<String, String>,
    entities: Vec<EntityCandidate>,
}

impl OpenApiImporter {
    fn new(surface: &SourceModelManifestSurface, document: OpenApiDocument) -> Self {
        Self {
            surface_id: surface.id.clone(),
            base_url: surface.base_url.raw().to_string(),
            document,
            diagnostics: Vec::new(),
            types: Vec::new(),
            type_ids: HashSet::new(),
            operations: Vec::new(),
            operation_ids: HashMap::new(),
            entities: Vec::new(),
        }
    }

    fn import(mut self) -> Result<OpenApiImportResult> {
        self.import_component_schemas();
        self.import_paths()?;

        let ir = SourceModelIr {
            ir_version: SOURCE_MODEL_IR_VERSION,
            surfaces: vec![SourceModelSurface {
                id: self.surface_id,
                description: String::new(),
                protocol: SurfaceProtocol::Rest,
                base_url: Some(self.base_url),
            }],
            types: self.types,
            operations: self.operations,
            entities: self.entities,
        };
        ir.validate(&[])?;
        Ok(OpenApiImportResult {
            ir,
            diagnostics: self.diagnostics,
        })
    }

    fn import_component_schemas(&mut self) {
        let Some(schemas) = self.document.object_at(&["components", "schemas"]) else {
            return;
        };
        let entries = sorted_map_entries_cloned(schemas);
        for (name, schema) in entries {
            let type_id = component_type_id(&name);
            if self.type_ids.contains(&type_id) {
                self.diagnostic(
                    format!("#/components/schemas/{name}"),
                    format!("schema component '{name}' is declared more than once"),
                );
                continue;
            }
            if let Some(definition) = self.schema_to_definition(
                &type_id,
                &name,
                &schema,
                &format!("#/components/schemas/{name}"),
            ) {
                let is_entity_candidate = matches!(
                    definition.kind,
                    TypeDefinitionKind::Object { .. } | TypeDefinitionKind::Interface { .. }
                );
                self.add_type(definition);
                if is_entity_candidate {
                    self.entities.push(EntityCandidate {
                        surface: self.surface_id.clone(),
                        entity: type_id,
                        keys: Vec::<IdentityKey>::new(),
                    });
                }
            }
        }
    }

    fn import_paths(&mut self) -> Result<()> {
        let Some(paths) = self.document.object_at(&["paths"]) else {
            self.diagnostic("#/paths", "OpenAPI document has no paths mapping");
            return Ok(());
        };
        let entries = sorted_map_entries_cloned(paths);
        for (path, path_item) in entries {
            let Some(path_item) = self.resolved_object(&path_item, &format!("#/paths/{path}"))
            else {
                self.diagnostic(
                    format!("#/paths/{path}"),
                    "path item is not a supported mapping",
                );
                continue;
            };
            let path_parameters = path_item
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for method in SUPPORTED_METHODS {
                let Some(operation_value) = path_item.get(method.openapi_key) else {
                    continue;
                };
                let Some(operation) = self.resolved_object(
                    operation_value,
                    &format!("#/paths/{path}/{}", method.openapi_key),
                ) else {
                    self.diagnostic(
                        format!("#/paths/{path}/{}", method.openapi_key),
                        "operation is not a supported mapping",
                    );
                    continue;
                };
                self.import_operation(&path, method, &path_parameters, &operation)?;
            }
        }
        Ok(())
    }

    fn import_operation(
        &mut self,
        path: &str,
        method: &MethodSpec,
        path_parameters: &[Value],
        operation: &Map<String, Value>,
    ) -> Result<()> {
        let operation_id = operation
            .get("operationId")
            .and_then(Value::as_str)
            .map(normalize_operation_id)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| fallback_operation_id(method.openapi_key, path));
        if let Some(existing) = self.operation_ids.insert(
            operation_id.clone(),
            format!("{} {path}", method.method_name),
        ) {
            return Err(ManifestError::validation(format!(
                "OpenAPI surface '{}' has duplicate operation id '{}' for {} and {} {}",
                self.surface_id, operation_id, existing, method.method_name, path
            )));
        }

        let mut inputs = Vec::new();
        let mut input_names = HashSet::new();
        let mut rest_parameters = Vec::new();
        self.import_parameters(
            path,
            &operation_id,
            path_parameters,
            &mut inputs,
            &mut input_names,
            &mut rest_parameters,
        );
        if let Some(parameters) = operation.get("parameters").and_then(Value::as_array) {
            self.import_parameters(
                path,
                &operation_id,
                parameters,
                &mut inputs,
                &mut input_names,
                &mut rest_parameters,
            );
        }
        let request_body =
            self.import_request_body(&operation_id, operation, &mut inputs, &mut input_names);
        let (result, responses) = self.import_responses(&operation_id, operation);
        let pagination = pagination_from_inputs(&inputs, &result);

        self.operations.push(SourceModelOperation {
            id: operation_id,
            surface: self.surface_id.clone(),
            description: operation
                .get("summary")
                .or_else(|| operation.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            inputs,
            result,
            details: OperationDetails::Rest {
                rest: Box::new(RestOperationDetails {
                    method: method.http_method,
                    path: path.to_string(),
                    parameters: rest_parameters,
                    request_body,
                    responses,
                    pagination,
                }),
            },
        });
        Ok(())
    }

    fn import_parameters(
        &mut self,
        path: &str,
        operation_id: &str,
        parameters: &[Value],
        inputs: &mut Vec<OperationInput>,
        input_names: &mut HashSet<String>,
        rest_parameters: &mut Vec<RestParameter>,
    ) {
        for parameter in parameters {
            let location = format!("#/paths/{path}/parameters");
            let Some(parameter) = self.resolved_object(parameter, &location) else {
                self.diagnostic(location, "parameter is not a supported mapping");
                continue;
            };
            let Some(name) = parameter.get("name").and_then(Value::as_str) else {
                self.diagnostic("#/parameters", "parameter is missing name");
                continue;
            };
            let Some(raw_location) = parameter.get("in").and_then(Value::as_str) else {
                self.diagnostic(format!("#/parameters/{name}"), "parameter is missing in");
                continue;
            };
            let Some(location) = parameter_location(raw_location) else {
                self.diagnostic(
                    format!("#/parameters/{name}"),
                    format!("unsupported parameter location '{raw_location}'"),
                );
                continue;
            };
            let input_name = name.to_string();
            let required = parameter
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(matches!(location, RestParameterLocation::Path));
            let schema = parameter.get("schema").unwrap_or(&Value::Null);
            let ty = self.schema_to_type_ref(
                schema,
                &format!("{operation_id}.parameter.{input_name}"),
                &format!("#/parameters/{name}/schema"),
            );
            self.add_operation_input(
                inputs,
                input_names,
                OperationInput {
                    name: input_name.clone(),
                    ty,
                    required,
                    description: parameter
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                },
                &format!("operation '{operation_id}' parameter '{name}'"),
            );
            rest_parameters.push(RestParameter {
                name: name.to_string(),
                input: input_name,
                location,
                required,
                style: parameter
                    .get("style")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                explode: parameter.get("explode").and_then(Value::as_bool),
            });
        }
    }

    fn import_request_body(
        &mut self,
        operation_id: &str,
        operation: &Map<String, Value>,
        inputs: &mut Vec<OperationInput>,
        input_names: &mut HashSet<String>,
    ) -> Option<RestRequestBody> {
        let request_body_value = operation.get("requestBody")?;
        let request_body = self.resolved_object(
            request_body_value,
            &format!("#/operations/{operation_id}/requestBody"),
        )?;
        let Some((content_type, media_type)) = select_media_type(&request_body) else {
            self.diagnostic(
                format!("#/operations/{operation_id}/requestBody/content"),
                "request body has no supported content entry",
            );
            return None;
        };
        let schema = media_type.get("schema").cloned().unwrap_or(Value::Null);
        let input_name = if input_names.contains("body") {
            "request_body".to_string()
        } else {
            "body".to_string()
        };
        let ty = self.schema_to_type_ref(
            &schema,
            &format!("{operation_id}.request_body"),
            &format!("#/operations/{operation_id}/requestBody/content/{content_type}/schema"),
        );
        let required = request_body
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.add_operation_input(
            inputs,
            input_names,
            OperationInput {
                name: input_name.clone(),
                ty,
                required,
                description: request_body
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            &format!("operation '{operation_id}' request body"),
        );
        Some(RestRequestBody {
            input: input_name,
            content_type,
            required,
        })
    }

    fn import_responses(
        &mut self,
        operation_id: &str,
        operation: &Map<String, Value>,
    ) -> (OperationResult, Vec<RestResponse>) {
        let Some(responses) = operation.get("responses").and_then(Value::as_object) else {
            self.diagnostic(
                format!("#/operations/{operation_id}/responses"),
                "operation has no responses mapping",
            );
            return (OperationResult::Unit, Vec::new());
        };
        let mut rest_responses = Vec::new();
        let mut success_result = None;
        for (status, response_value) in sorted_map_entries(responses) {
            let Some(response) = self.resolved_object(
                response_value,
                &format!("#/operations/{operation_id}/responses/{status}"),
            ) else {
                self.diagnostic(
                    format!("#/operations/{operation_id}/responses/{status}"),
                    "response is not a supported mapping",
                );
                continue;
            };
            let selected = select_media_type(&response);
            let mut body_content_type = None;
            let (body, body_path, result) = if let Some((content_type, media_type)) = selected {
                body_content_type = Some(content_type.clone());
                let schema = media_type.get("schema").cloned().unwrap_or(Value::Null);
                let response_ref = self.schema_to_type_ref(
                    &schema,
                    &format!("{operation_id}.response.{status}"),
                    &format!("#/operations/{operation_id}/responses/{status}/content/{content_type}/schema"),
                );
                let result = result_from_schema(&schema, response_ref.clone());
                let body_path = match &result {
                    OperationResult::WrappedList { items_path, .. } => items_path.clone(),
                    OperationResult::Unit
                    | OperationResult::Single { .. }
                    | OperationResult::List { .. } => Vec::new(),
                };
                (Some(response_ref), body_path, result)
            } else {
                (None, Vec::new(), OperationResult::Unit)
            };
            let is_success = is_success_status(status);
            if is_success && success_result.is_none() {
                success_result = Some(result);
            }
            rest_responses.push(RestResponse {
                status: parse_status_code(status),
                content_type: body_content_type,
                body,
                body_path,
                error: !is_success,
            });
        }
        (
            success_result.unwrap_or(OperationResult::Unit),
            rest_responses,
        )
    }

    fn schema_to_definition(
        &mut self,
        type_id: &str,
        name: &str,
        schema: &Value,
        location: &str,
    ) -> Option<TypeDefinition> {
        let schema = self.resolved_schema(schema, location)?;
        if enum_values(&schema).is_some() {
            let values = enum_values(&schema)
                .unwrap_or_default()
                .into_iter()
                .map(|name| EnumValue {
                    name,
                    description: String::new(),
                    deprecated: false,
                })
                .collect();
            return Some(TypeDefinition {
                id: type_id.to_string(),
                name: name.to_string(),
                description: description(&schema),
                kind: TypeDefinitionKind::Enum { values },
            });
        }
        match schema_type(&schema).as_deref() {
            Some("object") | None if schema.get("properties").is_some() => {
                let fields = self.object_fields(&schema, location);
                Some(TypeDefinition {
                    id: type_id.to_string(),
                    name: name.to_string(),
                    description: description(&schema),
                    kind: TypeDefinitionKind::Object { fields },
                })
            }
            Some("string" | "integer" | "number" | "boolean") => Some(TypeDefinition {
                id: type_id.to_string(),
                name: name.to_string(),
                description: description(&schema),
                kind: TypeDefinitionKind::Scalar {
                    scalar: scalar_type(&schema),
                },
            }),
            Some(other) => {
                self.diagnostic(
                    location,
                    format!("unsupported component schema type '{other}'"),
                );
                None
            }
            None => {
                self.diagnostic(location, "unsupported component schema without type");
                None
            }
        }
    }

    fn schema_to_type_ref(
        &mut self,
        schema: &Value,
        inline_type_id: &str,
        location: &str,
    ) -> TypeRef {
        if let Some(ref_value) = schema.get("$ref").and_then(Value::as_str) {
            if let Some(name) = ref_value.strip_prefix("#/components/schemas/") {
                let type_id = component_type_id(name);
                let Some(resolved) = self.document.resolve_ref_cloned(ref_value) else {
                    self.diagnostic(
                        location,
                        format!("could not resolve schema reference '{ref_value}'"),
                    );
                    return TypeRef {
                        nullable: false,
                        kind: TypeRefKind::Any,
                    };
                };
                if component_schema_defines_named_type(&resolved) {
                    return TypeRef::named(type_id);
                }
                return self.schema_to_type_ref(&resolved, &type_id, location);
            }
            self.diagnostic(
                location,
                format!("unsupported schema reference '{ref_value}'"),
            );
            return TypeRef {
                nullable: false,
                kind: TypeRefKind::Any,
            };
        }
        let Some(schema) = self.resolved_schema(schema, location) else {
            return TypeRef {
                nullable: false,
                kind: TypeRefKind::Any,
            };
        };
        let nullable = schema
            .get("nullable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if enum_values(&schema).is_some() && !inline_type_id.is_empty() {
            let type_id = unique_inline_type_id(inline_type_id);
            if !self.type_ids.contains(&type_id) {
                let values = enum_values(&schema)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|name| EnumValue {
                        name,
                        description: String::new(),
                        deprecated: false,
                    })
                    .collect();
                self.add_type(TypeDefinition {
                    id: type_id.clone(),
                    name: type_id.clone(),
                    description: description(&schema),
                    kind: TypeDefinitionKind::Enum { values },
                });
            }
            return TypeRef {
                nullable,
                kind: TypeRefKind::Named { type_id },
            };
        }
        match schema_type(&schema).as_deref() {
            Some("array") => {
                let item = schema.get("items").map_or_else(
                    || TypeRef {
                        nullable: false,
                        kind: TypeRefKind::Any,
                    },
                    |items| {
                        self.schema_to_type_ref(
                            items,
                            &format!("{inline_type_id}.item"),
                            &format!("{location}/items"),
                        )
                    },
                );
                TypeRef {
                    nullable,
                    kind: TypeRefKind::List {
                        item: Box::new(item),
                    },
                }
            }
            Some("object") | None if schema.get("properties").is_some() => {
                let type_id = unique_inline_type_id(inline_type_id);
                if !self.type_ids.contains(&type_id) {
                    let fields = self.object_fields(&schema, location);
                    self.add_type(TypeDefinition {
                        id: type_id.clone(),
                        name: type_id.clone(),
                        description: description(&schema),
                        kind: TypeDefinitionKind::Object { fields },
                    });
                }
                TypeRef {
                    nullable,
                    kind: TypeRefKind::Named { type_id },
                }
            }
            Some("object") if schema.get("additionalProperties").is_some() => {
                let value = schema
                    .get("additionalProperties")
                    .filter(|value| value.is_object())
                    .map_or_else(
                        || TypeRef::scalar(ScalarType::Json),
                        |value| {
                            self.schema_to_type_ref(
                                value,
                                &format!("{inline_type_id}.value"),
                                &format!("{location}/additionalProperties"),
                            )
                        },
                    );
                TypeRef {
                    nullable,
                    kind: TypeRefKind::Map {
                        value: Box::new(value),
                    },
                }
            }
            Some("string" | "integer" | "number" | "boolean") => TypeRef {
                nullable,
                kind: TypeRefKind::Scalar {
                    scalar: scalar_type(&schema),
                },
            },
            Some(other) => {
                self.diagnostic(location, format!("unsupported schema type '{other}'"));
                TypeRef {
                    nullable,
                    kind: TypeRefKind::Any,
                }
            }
            None => TypeRef {
                nullable,
                kind: TypeRefKind::Any,
            },
        }
    }

    fn object_fields(&mut self, schema: &Value, location: &str) -> Vec<ObjectField> {
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return Vec::new();
        };
        sorted_map_entries(properties)
            .into_iter()
            .map(|(name, property_schema)| ObjectField {
                name: name.to_string(),
                ty: self.schema_to_type_ref(
                    property_schema,
                    &format!("{location}.{name}"),
                    &format!("{location}/properties/{name}"),
                ),
                description: property_schema
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments: Vec::<OperationInput>::new(),
                deprecated: property_schema
                    .get("deprecated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .map(|mut field| {
                if !required.contains(&field.name) {
                    field.ty.nullable = true;
                }
                field
            })
            .collect()
    }

    fn resolved_schema(&mut self, schema: &Value, location: &str) -> Option<Value> {
        if let Some(ref_value) = schema.get("$ref").and_then(Value::as_str) {
            let Some(resolved) = self.document.resolve_ref_cloned(ref_value) else {
                self.diagnostic(
                    location,
                    format!("could not resolve schema reference '{ref_value}'"),
                );
                return None;
            };
            return Some(resolved);
        }
        for keyword in ["oneOf", "anyOf", "allOf"] {
            if schema.get(keyword).is_some() {
                self.diagnostic(
                    location,
                    format!("schema composition '{keyword}' is not imported in this wave"),
                );
                return None;
            }
        }
        Some(schema.clone())
    }

    fn resolved_object(&mut self, value: &Value, location: &str) -> Option<Map<String, Value>> {
        let resolved = if let Some(ref_value) = value.get("$ref").and_then(Value::as_str) {
            let Some(resolved) = self.document.resolve_ref_cloned(ref_value) else {
                self.diagnostic(
                    location,
                    format!("could not resolve reference '{ref_value}'"),
                );
                return None;
            };
            resolved
        } else {
            value.clone()
        };
        resolved.as_object().cloned()
    }

    fn add_operation_input(
        &mut self,
        inputs: &mut Vec<OperationInput>,
        input_names: &mut HashSet<String>,
        input: OperationInput,
        context: &str,
    ) {
        if !input_names.insert(input.name.clone()) {
            self.diagnostic(
                context,
                format!("duplicate input '{}' was ignored", input.name),
            );
            return;
        }
        inputs.push(input);
    }

    fn add_type(&mut self, definition: TypeDefinition) {
        self.type_ids.insert(definition.id.clone());
        self.types.push(definition);
    }

    fn diagnostic(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.diagnostics.push(OpenApiImportDiagnostic {
            location: location.into(),
            message: message.into(),
        });
    }
}

struct MethodSpec {
    openapi_key: &'static str,
    method_name: &'static str,
    http_method: HttpMethod,
}

const SUPPORTED_METHODS: &[MethodSpec] = &[
    MethodSpec {
        openapi_key: "get",
        method_name: "GET",
        http_method: HttpMethod::GET,
    },
    MethodSpec {
        openapi_key: "post",
        method_name: "POST",
        http_method: HttpMethod::POST,
    },
    MethodSpec {
        openapi_key: "put",
        method_name: "PUT",
        http_method: HttpMethod::PUT,
    },
    MethodSpec {
        openapi_key: "patch",
        method_name: "PATCH",
        http_method: HttpMethod::PATCH,
    },
    MethodSpec {
        openapi_key: "delete",
        method_name: "DELETE",
        http_method: HttpMethod::DELETE,
    },
];

fn sorted_map_entries(map: &Map<String, Value>) -> Vec<(&str, &Value)> {
    let mut entries = map
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect::<Vec<_>>();
    entries.sort_by_key(|(key, _)| *key);
    entries
}

fn sorted_map_entries_cloned(map: &Map<String, Value>) -> Vec<(String, Value)> {
    let mut entries = map
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn component_type_id(name: &str) -> String {
    name.trim().to_string()
}

fn component_schema_defines_named_type(schema: &Value) -> bool {
    if enum_values(schema).is_some() {
        return true;
    }
    matches!(
        schema_type(schema).as_deref(),
        Some("object") | None if schema.get("properties").is_some()
    ) || matches!(
        schema_type(schema).as_deref(),
        Some("string" | "integer" | "number" | "boolean")
    )
}

fn unique_inline_type_id(id: &str) -> String {
    if id.trim().is_empty() {
        "inline".to_string()
    } else {
        id.trim().to_string()
    }
}

fn normalize_operation_id(operation_id: &str) -> String {
    operation_id.trim().replace(char::is_whitespace, "_")
}

fn fallback_operation_id(method: &str, path: &str) -> String {
    let mut parts = vec![method.to_ascii_lowercase()];
    for segment in path.split('/') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let segment = segment.trim_start_matches('{').trim_end_matches('}');
        let slug = segment
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '.'
                }
            })
            .collect::<String>();
        for part in slug.split('.').filter(|part| !part.is_empty()) {
            parts.push(part.to_string());
        }
    }
    parts.join(".")
}

fn parameter_location(raw: &str) -> Option<RestParameterLocation> {
    match raw {
        "path" => Some(RestParameterLocation::Path),
        "query" => Some(RestParameterLocation::Query),
        "header" => Some(RestParameterLocation::Header),
        "cookie" => Some(RestParameterLocation::Cookie),
        _ => None,
    }
}

fn select_media_type(
    response_or_body: &Map<String, Value>,
) -> Option<(String, &Map<String, Value>)> {
    let content = response_or_body.get("content")?.as_object()?;
    if let Some(media_type) = content.get("application/json").and_then(Value::as_object) {
        return Some(("application/json".to_string(), media_type));
    }
    let mut json_entries = sorted_map_entries(content)
        .into_iter()
        .filter(|(content_type, _)| content_type.contains("json"))
        .collect::<Vec<_>>();
    if let Some((content_type, value)) = json_entries.pop() {
        return value
            .as_object()
            .map(|media_type| (content_type.to_string(), media_type));
    }
    sorted_map_entries(content)
        .into_iter()
        .find_map(|(content_type, value)| {
            value
                .as_object()
                .map(|media_type| (content_type.to_string(), media_type))
        })
}

fn schema_type(schema: &Value) -> Option<String> {
    match schema.get("type") {
        Some(Value::String(raw)) => Some(raw.clone()),
        Some(Value::Array(types)) => types
            .iter()
            .find_map(Value::as_str)
            .filter(|ty| *ty != "null")
            .map(ToString::to_string),
        _ => None,
    }
}

fn scalar_type(schema: &Value) -> ScalarType {
    match schema_type(schema).as_deref() {
        Some("integer") => ScalarType::Integer,
        Some("number") => ScalarType::Float,
        Some("boolean") => ScalarType::Boolean,
        Some("string") => match schema.get("format").and_then(Value::as_str) {
            Some("date-time" | "date") => ScalarType::Timestamp,
            Some("uri" | "url") => ScalarType::Uri,
            _ => ScalarType::String,
        },
        _ => ScalarType::Json,
    }
}

fn enum_values(schema: &Value) -> Option<Vec<String>> {
    let values = schema
        .get("enum")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn description(schema: &Value) -> String {
    schema
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn result_from_schema(schema: &Value, response_ref: TypeRef) -> OperationResult {
    if let Some("array") = schema_type(schema).as_deref() {
        let item = match response_ref.kind {
            TypeRefKind::List { item } => *item,
            _ => TypeRef {
                nullable: false,
                kind: TypeRefKind::Any,
            },
        };
        return OperationResult::List { item };
    }
    let wrapped_items = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get("items"))
        .filter(|items| matches!(schema_type(items).as_deref(), Some("array")));
    if let Some(items) = wrapped_items {
        let item = items
            .get("items")
            .and_then(|schema| schema.get("$ref").and_then(Value::as_str))
            .and_then(|ref_value| ref_value.strip_prefix("#/components/schemas/"))
            .map(component_type_id)
            .map_or(
                TypeRef {
                    nullable: false,
                    kind: TypeRefKind::Any,
                },
                TypeRef::named,
            );
        let total_count_path = schema
            .get("properties")
            .and_then(Value::as_object)
            .filter(|properties| properties.contains_key("total_count"))
            .map(|_| vec!["total_count".to_string()])
            .unwrap_or_default();
        return OperationResult::WrappedList {
            item,
            items_path: vec!["items".to_string()],
            total_count_path,
        };
    }
    OperationResult::Single { ty: response_ref }
}

fn parse_status_code(status: &str) -> RestStatusCode {
    if let Ok(code) = status.parse::<u16>() {
        RestStatusCode::Code(code)
    } else if status.eq_ignore_ascii_case("default") {
        RestStatusCode::Default(status.to_string())
    } else {
        RestStatusCode::Range(status.to_string())
    }
}

fn is_success_status(status: &str) -> bool {
    status.parse::<u16>().map_or_else(
        |_| status.starts_with('2'),
        |code| (200..300).contains(&code),
    )
}

fn pagination_from_inputs(
    inputs: &[OperationInput],
    result: &OperationResult,
) -> Option<RestPagination> {
    if !matches!(
        result,
        OperationResult::List { .. } | OperationResult::WrappedList { .. }
    ) {
        return None;
    }
    let input_names = inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect::<HashSet<_>>();
    if input_names.contains("per_page") {
        Some(RestPagination::LinkHeader {
            next_rel: Some("next".to_string()),
            page_size_input: Some("per_page".to_string()),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{import_openapi_surface, sha256_hex};
    use crate::backends::http::{AuthSpec, RateLimitSpec};
    use crate::backends::source_model::{SourceModelManifestSurface, SurfaceDescriptionType};
    use crate::{
        HttpMethod, OperationDetails, OperationResult, ParsedTemplate, RestPagination,
        RestParameterLocation, TypeRefKind,
    };

    const GITHUB_SHAPED_OPENAPI: &str = r"
openapi: 3.0.3
info:
  title: GitHub shaped fixture
  version: 1.0.0
paths:
  /repos/{owner}/{repo}/issues:
    parameters:
      - name: owner
        in: path
        required: true
        schema: { type: string }
      - name: repo
        in: path
        required: true
        schema: { type: string }
    get:
      operationId: issues/list-for-repo
      summary: List repository issues
      parameters:
        - name: state
          in: query
          schema:
            type: string
            enum: [open, closed, all]
        - name: per_page
          in: query
          schema: { type: integer }
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: '#/components/schemas/Issue'
    post:
      operationId: issues/create
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/IssueCreate'
      responses:
        '201':
          description: Created
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Issue'
  /repos/{owner}/{repo}/issues/{issue_number}:
    get:
      operationId: issues/get
      parameters:
        - name: owner
          in: path
          required: true
          schema: { type: string }
        - name: repo
          in: path
          required: true
          schema: { type: string }
        - name: issue_number
          in: path
          required: true
          schema: { type: integer }
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Issue'
  /repos/{owner}/{repo}/issues/{issue_number}/lock:
    put:
      operationId: issues/lock
      parameters:
        - name: owner
          in: path
          required: true
          schema: { type: string }
        - name: repo
          in: path
          required: true
          schema: { type: string }
        - name: issue_number
          in: path
          required: true
          schema: { type: integer }
      responses:
        '204':
          description: No Content
  /search/issues:
    get:
      operationId: search/issues
      parameters:
        - name: q
          in: query
          required: true
          schema: { type: string }
        - name: per_page
          in: query
          schema: { type: integer }
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                type: object
                required: [items]
                properties:
                  total_count: { type: integer }
                  items:
                    type: array
                    items:
                      $ref: '#/components/schemas/Issue'
  /zen:
    get:
      responses:
        '200':
          description: OK
          content:
            text/plain:
              schema: { type: string }
components:
  schemas:
    Issue:
      type: object
      required: [id, number, title]
      properties:
        id: { type: integer }
        number: { type: integer }
        title: { type: string }
        html_url: { type: string, format: uri }
    IssueCreate:
      type: object
      required: [title]
      properties:
        title: { type: string }
        body: { type: string }
";

    #[test]
    fn importer_imports_github_shaped_rest_slice() {
        let surface = pinned_surface(GITHUB_SHAPED_OPENAPI);
        let result = import_openapi_surface(&surface, GITHUB_SHAPED_OPENAPI.as_bytes())
            .expect("GitHub-shaped OpenAPI should import");

        assert!(
            result.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            result.diagnostics
        );
        result
            .ir
            .validate(&[])
            .expect("imported IR should validate");
        assert!(
            result
                .ir
                .entities
                .iter()
                .any(|entity| { entity.surface == "github-rest" && entity.entity == "Issue" })
        );

        let list = operation(&result.ir.operations, "issues/list-for-repo");
        assert!(matches!(list.result, OperationResult::List { .. }));
        let OperationDetails::Rest { rest } = &list.details else {
            panic!("list operation should be REST");
        };
        assert_eq!(rest.method, HttpMethod::GET);
        assert_eq!(rest.path, "/repos/{owner}/{repo}/issues");
        assert!(rest.parameters.iter().any(|parameter| {
            parameter.name == "owner" && parameter.location == RestParameterLocation::Path
        }));
        assert!(rest.parameters.iter().any(|parameter| {
            parameter.name == "state" && parameter.location == RestParameterLocation::Query
        }));
        assert!(matches!(
            rest.pagination,
            Some(RestPagination::LinkHeader {
                page_size_input: Some(ref input),
                ..
            }) if input == "per_page"
        ));

        let search = operation(&result.ir.operations, "search/issues");
        assert!(matches!(
            search.result,
            OperationResult::WrappedList {
                ref items_path,
                ref total_count_path,
                ..
            } if items_path == &["items".to_string()]
                && total_count_path == &["total_count".to_string()]
        ));

        let create = operation(&result.ir.operations, "issues/create");
        assert!(matches!(create.result, OperationResult::Single { .. }));
        let OperationDetails::Rest { rest } = &create.details else {
            panic!("create operation should be REST");
        };
        assert_eq!(rest.method, HttpMethod::POST);
        let body = rest
            .request_body
            .as_ref()
            .expect("create operation should have a request body");
        assert_eq!(body.input, "body");
        assert_eq!(body.content_type, "application/json");

        let lock = operation(&result.ir.operations, "issues/lock");
        assert!(matches!(lock.result, OperationResult::Unit));
        let OperationDetails::Rest { rest } = &lock.details else {
            panic!("lock operation should be REST");
        };
        assert_eq!(rest.method, HttpMethod::PUT);

        let fallback = operation(&result.ir.operations, "get.zen");
        assert!(matches!(fallback.result, OperationResult::Single { .. }));
    }

    #[test]
    fn importer_inlines_component_array_references() {
        let document = r"
openapi: 3.0.3
info: { title: array component refs, version: 1.0.0 }
paths:
  /search/code:
    get:
      operationId: search/code
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/code-search-result-item'
components:
  schemas:
    code-search-result-item:
      type: object
      properties:
        name: { type: string }
        text_matches:
          $ref: '#/components/schemas/search-result-text-matches'
    search-result-text-matches:
      type: array
      items:
        type: object
        properties:
          object_url: { type: string }
          fragment: { type: string }
";
        let surface = pinned_surface(document);
        let result = import_openapi_surface(&surface, document.as_bytes())
            .expect("component array references should import");

        result
            .ir
            .validate(&[])
            .expect("imported IR should validate without dangling component array refs");
        assert!(
            !result
                .ir
                .types
                .iter()
                .any(|ty| ty.id == "search-result-text-matches"),
            "array components are represented as TypeRef lists, not named TypeDefinitions"
        );
        let code_result = result
            .ir
            .types
            .iter()
            .find(|ty| ty.id == "code-search-result-item")
            .expect("object component should be imported");
        let crate::TypeDefinitionKind::Object { fields } = &code_result.kind else {
            panic!("code search result item should be an object");
        };
        let text_matches = fields
            .iter()
            .find(|field| field.name == "text_matches")
            .expect("text_matches field should be imported");
        assert!(matches!(text_matches.ty.kind, TypeRefKind::List { .. }));
    }

    #[test]
    fn importer_treats_non_string_enum_values_as_scalars() {
        let document = r"
openapi: 3.0.3
info: { title: non-string enum values, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: events/list
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Event'
components:
  schemas:
    Event:
      type: object
      properties:
        locked:
          type: boolean
          enum: [true]
";
        let surface = pinned_surface(document);
        let result = import_openapi_surface(&surface, document.as_bytes())
            .expect("non-string enum values should not create empty enum types");

        result
            .ir
            .validate(&[])
            .expect("imported IR should validate");
        let event = result
            .ir
            .types
            .iter()
            .find(|ty| ty.id == "Event")
            .expect("event component should be imported");
        let crate::TypeDefinitionKind::Object { fields } = &event.kind else {
            panic!("event should be an object");
        };
        let locked = fields
            .iter()
            .find(|field| field.name == "locked")
            .expect("locked field should be imported");
        assert!(matches!(
            locked.ty.kind,
            TypeRefKind::Scalar {
                scalar: crate::ScalarType::Boolean
            }
        ));
    }

    #[test]
    fn importer_rejects_unpinned_or_mismatched_documents() {
        let mut surface = pinned_surface(GITHUB_SHAPED_OPENAPI);
        surface.sha256 = "0".repeat(64);
        let error = import_openapi_surface(&surface, GITHUB_SHAPED_OPENAPI.as_bytes())
            .expect_err("hash mismatch should fail");
        assert!(
            error.to_string().contains("sha256 mismatch"),
            "unexpected error: {error}"
        );

        surface.url.clear();
        let error = import_openapi_surface(&surface, GITHUB_SHAPED_OPENAPI.as_bytes())
            .expect_err("missing url should fail");
        assert!(
            error
                .to_string()
                .contains("must provide both url and sha256"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn importer_rejects_duplicate_surface_scoped_operation_ids() {
        let document = r"
openapi: 3.0.3
info: { title: duplicate ids, version: 1.0.0 }
paths:
  /one:
    get:
      operationId: duplicate/op
      responses:
        '204': { description: No Content }
  /two:
    get:
      operationId: duplicate/op
      responses:
        '204': { description: No Content }
";
        let surface = pinned_surface(document);
        let error = import_openapi_surface(&surface, document.as_bytes())
            .expect_err("duplicate operation IDs should fail");

        assert!(
            error
                .to_string()
                .contains("duplicate operation id 'duplicate/op'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn importer_emits_diagnostics_for_unsupported_constructs() {
        let document = r"
openapi: 3.0.3
info: { title: diagnostics, version: 1.0.0 }
paths:
  /diagnostics:
    get:
      parameters:
        - name: odd
          in: matrix
          schema: { type: string }
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                oneOf:
                  - { type: string }
                  - { type: integer }
";
        let surface = pinned_surface(document);
        let result = import_openapi_surface(&surface, document.as_bytes())
            .expect("unsupported non-critical constructs should not fail import");

        assert_eq!(result.ir.operations.len(), 1);
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unsupported parameter location")
        }));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("schema composition 'oneOf'") })
        );
        let imported = result
            .ir
            .operations
            .first()
            .expect("operation should be imported");
        assert_eq!(imported.id, "get.diagnostics");
        assert!(matches!(imported.result, OperationResult::Single { ref ty }
            if matches!(ty.kind, TypeRefKind::Any)));
    }

    fn pinned_surface(document: &str) -> SourceModelManifestSurface {
        SourceModelManifestSurface {
            id: "github-rest".to_string(),
            surface_type: SurfaceDescriptionType::OpenApi,
            url: "https://example.com/openapi.yaml".to_string(),
            sha256: sha256_hex(document.as_bytes()),
            base_url: ParsedTemplate::parse("https://api.github.com")
                .expect("base URL template should parse"),
            auth: AuthSpec::default(),
            request_headers: Vec::new(),
            rate_limit: RateLimitSpec::default(),
        }
    }

    fn operation<'a>(
        operations: &'a [crate::SourceModelOperation],
        id: &str,
    ) -> &'a crate::SourceModelOperation {
        operations
            .iter()
            .find(|operation| operation.id == id)
            .unwrap_or_else(|| panic!("missing operation {id}"))
    }
}
