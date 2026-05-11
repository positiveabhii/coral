//! Types for modelling source models

#![allow(
    dead_code,
    reason = "Source-model prototype types are not wired into manifest parsing or runtime registration yet."
)]
#![allow(
    missing_docs,
    reason = "Source-model prototype types are being made public incrementally during spike work."
)]

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
            ty,
            required: true,
        }
    }

    fn optional(name: &str, ty: TypeRef) -> OperationInput {
        OperationInput {
            name: name.to_string(),
            ty,
            required: false,
        }
    }
}

/// Identifier for a logical source operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationId(String);

impl OperationId {
    /// Creates an operation identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
pub struct SurfaceId(String);

impl SurfaceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    id: SurfaceId,     // "github.rest"
    kind: SurfaceKind, // Rest
    base_url: String,  // "https://api.github.com"
    auth: Auth,
    headers: Vec<Header>,
}

impl Surface {
    pub fn id(&self) -> &SurfaceId {
        &self.id
    }

    pub fn kind(&self) -> &SurfaceKind {
        &self.kind
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    pub fn headers(&self) -> &[Header] {
        &self.headers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceKind {
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    BearerToken {
        input: InputId, // "GITHUB_TOKEN"
        header: String, // "Authorization"
        prefix: String, // "Bearer "
    },
}

impl Auth {
    pub fn bearer_token_parts(&self) -> (&InputId, &str, &str) {
        match self {
            Self::BearerToken {
                input,
                header,
                prefix,
            } => (input, header, prefix),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputId(String);

impl InputId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    name: String,
    value: HeaderValue,
}

impl Header {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &HeaderValue {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    Literal(String),
    Input(InputId),
    // TODO: add a FromTemplate option?
}

// ----- Binding ---------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BindingId(String);

impl BindingId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    id: BindingId,          // "github.issue.list.http"
    operation: OperationId, // "github.issue.list"
    surface: SurfaceId,     // "github.rest"
    protocol: BindingProtocol,
}

impl Binding {
    pub fn id(&self) -> &BindingId {
        &self.id
    }

    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    pub fn surface(&self) -> &SurfaceId {
        &self.surface
    }

    pub fn protocol(&self) -> &BindingProtocol {
        &self.protocol
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingProtocol {
    Http(HttpBinding),
}

impl BindingProtocol {
    pub fn as_http(&self) -> &HttpBinding {
        match self {
            Self::Http(binding) => binding,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pagination {
    LinkHeader { page_size: PageSize },
}

impl Pagination {
    pub fn page_size(&self) -> &PageSize {
        match self {
            Self::LinkHeader { page_size } => page_size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSize {
    query_param: String,
    default: u32,
    max: u32,
}

impl PageSize {
    pub fn query_param(&self) -> &str {
        &self.query_param
    }

    pub fn default(&self) -> u32 {
        self.default
    }

    pub fn max(&self) -> u32 {
        self.max
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpBinding {
    method: HttpMethod,
    path: String,
    query: Vec<QueryParamBinding>,
    response: ResponseBinding,
    pagination: Option<Pagination>,
}

impl HttpBinding {
    pub fn method(&self) -> HttpMethod {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> &[QueryParamBinding] {
        &self.query
    }

    pub fn response(&self) -> &ResponseBinding {
        &self.response
    }

    pub fn pagination(&self) -> Option<&Pagination> {
        self.pagination.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParamBinding {
    name: String,   // query param name, e.g. "state"
    input: InputId, // operation input, e.g. "state"
}

impl QueryParamBinding {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn input(&self) -> &InputId {
        &self.input
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseBinding {
    items_path: JsonPath,
}

impl ResponseBinding {
    pub fn items_path(&self) -> &JsonPath {
        &self.items_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPath(String);

impl JsonPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ----- Source Model ---------------------------------------

struct SourceModel {
    source: String, // e.g. "github"
    surfaces: Vec<Surface>,
    operations: Vec<Operation>,
    bindings: Vec<Binding>,
}

pub fn github_rest_surface() -> Surface {
    Surface {
        id: SurfaceId::new("github.rest"),
        base_url: "https://api.github.com".to_string(),
        kind: SurfaceKind::Rest,
        auth: Auth::BearerToken {
            input: InputId::new("GITHUB_TOKEN"),
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

pub fn github_issue_list_rest_binding() -> Binding {
    Binding {
        id: BindingId::new("github.issue.list.http"),
        operation: OperationId::new("github.issue.list"),
        surface: github_rest_surface().id,
        protocol: BindingProtocol::Http(HttpBinding {
            method: HttpMethod::Get,
            path: "/repos/{owner}/{repo}/issues".to_string(),
            query: vec![QueryParamBinding {
                name: "state".to_string(),
                input: InputId::new("state"),
            }],
            response: ResponseBinding {
                items_path: JsonPath("$".to_string()),
            },
            pagination: Some(Pagination::LinkHeader {
                page_size: PageSize {
                    query_param: "per_page".to_string(),
                    default: 100,
                    max: 100,
                },
            }),
        }),
    }
}

// ----- Tests ---------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn gt_issue_entity() -> EntityId {
        EntityId("github.issue".into())
    }

    fn gh_issue_list_op() -> Operation {
        Operation {
            id: OperationId::new("github.issue.list"),
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

    fn gh_source_model() -> SourceModel {
        SourceModel {
            source: "github".to_string(),
            surfaces: vec![github_rest_surface()],
            operations: vec![gh_issue_list_op()],
            bindings: vec![github_issue_list_rest_binding()],
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

    #[test]
    fn github_issues_rest_binding_matches_spike_request_shape() {
        let binding = github_issue_list_rest_binding();
        let http = binding.protocol().as_http();

        assert_eq!(binding.operation().as_str(), "github.issue.list");
        assert_eq!(binding.surface().as_str(), "github.rest");
        assert_eq!(http.method(), HttpMethod::Get);
        assert_eq!(http.path(), "/repos/{owner}/{repo}/issues");
        assert_eq!(http.query()[0].name(), "state");
        assert_eq!(http.query()[0].input().as_str(), "state");

        let page_size = http
            .pagination()
            .expect("github issues should use link-header pagination")
            .page_size();
        assert_eq!(page_size.query_param(), "per_page");
        assert_eq!(page_size.default(), 100);
        assert_eq!(page_size.max(), 100);
    }
}
