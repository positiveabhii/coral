//! Types for modelling source models

#![allow(dead_code)]

// ----- Basic types ---------------------------------------

#[derive(Debug, PartialEq, Eq, Clone)]
enum ScalarType {
    /// Add docs
    String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeRef {
    Scalar(ScalarType),
    Entity(EntityId),
    Collection(Box<TypeRef>),
}

impl TypeRef {
    fn collection(inner: TypeRef) -> Self {
        Self::Collection(Box::new(inner))
    }
}

// ----- Operations ---------------------------------------

#[derive(Debug, PartialEq)]
enum OperationKind {
    List,
}

#[derive(Debug)]
struct OperationInput {
    name: String,
    ty: TypeRef,
    required: bool,
}

impl OperationInput {
    fn required(name: &str, ty: TypeRef) -> OperationInput {
        OperationInput {
            name: name.to_string(),
            ty: ty,
            required: true,
        }
    }

    fn optional(name: &str, ty: TypeRef) -> OperationInput {
        OperationInput {
            name: name.to_string(),
            ty: ty,
            required: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OperationId(String);

#[derive(Debug)]
struct Operation {
    id: OperationId,
    kind: OperationKind,
    entity: EntityId,
    inputs: Vec<OperationInput>,
    returns: TypeRef,
}

// ----- Entities ---------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EntityId(String);

// ----- Surface ---------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SurfaceId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Surface {
    id: SurfaceId,     // "github.rest"
    kind: SurfaceKind, // Rest
    base_url: String,  // "https://api.github.com"
    auth: Auth,
    headers: Vec<Header>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SurfaceKind {
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Auth {
    BearerToken {
        input: InputId, // "GITHUB_TOKEN"
        header: String, // "Authorization"
        prefix: String, // "Bearer "
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InputId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Header {
    name: String,
    value: HeaderValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HeaderValue {
    Literal(String),
    Input(InputId),
    // TODO: add a FromTemplate option?
}

// ----- Binding ---------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BindingId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    id: BindingId,          // "github.issue.list.http"
    operation: OperationId, // "github.issue.list"
    surface: SurfaceId,     // "github.rest"
    protocol: BindingProtocol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BindingProtocol {
    Http(HttpBinding),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pagination {
    LinkHeader { page_size: PageSize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PageSize {
    query_param: String,
    default: u32,
    max: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpBinding {
    method: HttpMethod,
    path: String,
    query: Vec<QueryParamBinding>,
    response: ResponseBinding,
    pagination: Option<Pagination>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HttpMethod {
    Get,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryParamBinding {
    name: String,   // query param name, e.g. "state"
    input: InputId, // operation input, e.g. "state"
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResponseBinding {
    items_path: JsonPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonPath(String);

// ----- Source Model ---------------------------------------

struct SourceModel {
    source: String, // e.g. "github"
    surfaces: Vec<Surface>,
    operations: Vec<Operation>,
    bindings: Vec<Binding>,
}

// ----- Tests ---------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn gt_issue_entity() -> EntityId {
        EntityId("github.issue".into())
    }

    fn gh_rest_surface() -> Surface {
        Surface {
            id: SurfaceId("github.rest".into()),
            base_url: "https://api.github.com".to_string(),
            kind: SurfaceKind::Rest,
            auth: Auth::BearerToken {
                input: InputId("GITHUB_TOKEN".into()),
                header: "Authorization".to_string(),
                prefix: "Bearer ".to_string(),
            },
            headers: vec![
                Header {
                    name: "Accept".to_string(),
                    value: HeaderValue::Literal("application/vnd.github+json".into()),
                },
                Header {
                    name: "X-GitHub-Api-Version".to_string(),
                    value: HeaderValue::Literal("2022-11-28".to_string()),
                },
            ],
        }
    }

    fn gh_issue_list_op() -> Operation {
        Operation {
            id: OperationId("github.issue.list".into()),
            kind: OperationKind::List,
            entity: gt_issue_entity(),
            inputs: vec![
                OperationInput::required("owner", TypeRef::Scalar(ScalarType::String)),
                OperationInput::required("repo", TypeRef::Scalar(ScalarType::String)),
                OperationInput::optional("state", TypeRef::Scalar(ScalarType::String)),
            ],
            returns: TypeRef::collection(TypeRef::Entity(gt_issue_entity())),
        }
    }

    fn gh_issue_list_rest_binding() -> Binding {
        Binding {
            id: BindingId("github.issue.list.http".into()),
            operation: gh_issue_list_op().id,
            surface: gh_rest_surface().id,
            protocol: {
                BindingProtocol::Http(HttpBinding {
                    method: HttpMethod::Get,
                    // Codex comment: For path parameters, I’d let the path template reference operation inputs
                    // by name: `{owner}` and `{repo}`. If you want every mapping to be explicit, add a
                    // `path_params: Vec<PathParamBinding>`, but for this spike the template may be enough.
                    path: "/rpeos/{owner}/{repo}/issues".to_string(),
                    query: vec![QueryParamBinding {
                        name: "state".to_string(),
                        input: InputId("state".to_string()),
                    }],
                    response: ResponseBinding {
                        items_path: JsonPath("$".to_string()),
                    },
                    pagination: Some(Pagination::LinkHeader {
                        page_size: PageSize {
                            query_param: "per_page".to_string(),
                            default: 30,
                            max: 100,
                        },
                    }),
                })
            },
        }
    }

    fn gh_source_model() -> SourceModel {
        SourceModel {
            source: "github".to_string(),
            surfaces: vec![gh_rest_surface()],
            operations: vec![gh_issue_list_op()],
            bindings: vec![gh_issue_list_rest_binding()],
        }
    }

    #[test]
    fn source_model_can_be_created() {
        let gh_source_model = gh_source_model();

        assert_eq!(gh_source_model.operations.len(), 1);
        assert_eq!(gh_source_model.operations[0].kind, OperationKind::List);
        assert_eq!(gh_source_model.surfaces.len(), 1);
        assert_eq!(gh_source_model.surfaces[0].kind, SurfaceKind::Rest);
    }
}
