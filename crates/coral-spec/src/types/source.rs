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
pub enum ScalarType {
    String,
    Integer,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Scalar(ScalarType),
    Entity(EntityId),
    Collection(Box<TypeRef>),
}

impl TypeRef {
    pub fn collection(inner: TypeRef) -> Self {
        Self::Collection(Box::new(inner))
    }
}

// ----- Operations ---------------------------------------

#[derive(Debug, PartialEq)]
pub enum OperationKind {
    List,
}

#[derive(Debug)]
pub struct OperationInput {
    name: String,
    ty: TypeRef,
    required: bool,
}

impl OperationInput {
    pub fn required(name: &str, ty: TypeRef) -> OperationInput {
        OperationInput {
            name: name.to_string(),
            ty,
            required: true,
        }
    }

    pub fn optional(name: &str, ty: TypeRef) -> OperationInput {
        OperationInput {
            name: name.to_string(),
            ty,
            required: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> &TypeRef {
        &self.ty
    }

    pub fn is_required(&self) -> bool {
        self.required
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
pub struct Operation {
    id: OperationId,
    kind: OperationKind,
    entity: EntityId,
    inputs: Vec<OperationInput>,
}

impl Operation {
    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn kind(&self) -> &OperationKind {
        &self.kind
    }

    pub fn entity(&self) -> &EntityId {
        &self.entity
    }

    pub fn inputs(&self) -> &[OperationInput] {
        &self.inputs
    }
}

// ----- Entities ---------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityId(String);

impl EntityId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityField {
    name: String,
    ty: TypeRef,
    nullable: bool,
}

impl EntityField {
    pub fn new(name: impl Into<String>, ty: TypeRef, nullable: bool) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> &TypeRef {
        &self.ty
    }

    pub fn nullable(&self) -> bool {
        self.nullable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    id: EntityId,
    fields: Vec<EntityField>,
}

impl Entity {
    pub fn new(id: EntityId, fields: Vec<EntityField>) -> Self {
        Self { id, fields }
    }

    pub fn id(&self) -> &EntityId {
        &self.id
    }

    pub fn fields(&self) -> &[EntityField] {
        &self.fields
    }
}

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

pub struct SourceModel {
    source: String, // e.g. "github"
    entities: Vec<Entity>,
    surfaces: Vec<Surface>,
    operations: Vec<Operation>,
    bindings: Vec<Binding>,
}

impl SourceModel {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn surfaces(&self) -> &[Surface] {
        &self.surfaces
    }

    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    pub fn entity(&self, id: &EntityId) -> Option<&Entity> {
        self.entities.iter().find(|entity| entity.id() == id)
    }

    pub fn operation(&self, id: &OperationId) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|operation| operation.id() == id)
    }
}

pub fn github_user_entity() -> Entity {
    Entity::new(
        EntityId::new("github.user"),
        vec![
            EntityField::new("login", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("id", TypeRef::Scalar(ScalarType::Integer), false),
            EntityField::new("html_url", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("type", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("site_admin", TypeRef::Scalar(ScalarType::Boolean), false),
        ],
    )
}

pub fn github_issue_entity() -> Entity {
    Entity::new(
        EntityId::new("github.issue"),
        vec![
            EntityField::new("number", TypeRef::Scalar(ScalarType::Integer), false),
            EntityField::new("title", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("state", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("created_at", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("html_url", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("user", TypeRef::Entity(EntityId::new("github.user")), true),
        ],
    )
}

pub fn github_issue_list_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.list"),
        kind: OperationKind::List,
        entity: EntityId::new("github.issue"),
        inputs: vec![
            OperationInput::required("owner", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("repo", TypeRef::Scalar(ScalarType::String)),
            OperationInput::optional("state", TypeRef::Scalar(ScalarType::String)),
        ],
    }
}

pub fn github_issue_search_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.search"),
        kind: OperationKind::List,
        entity: EntityId::new("github.issue"),
        inputs: vec![
            OperationInput::required("q", TypeRef::Scalar(ScalarType::String)),
            OperationInput::optional("sort", TypeRef::Scalar(ScalarType::String)),
            OperationInput::optional("order", TypeRef::Scalar(ScalarType::String)),
        ],
    }
}

pub fn github_source_model() -> SourceModel {
    SourceModel {
        source: "github".to_string(),
        entities: vec![github_issue_entity(), github_user_entity()],
        surfaces: vec![github_rest_surface()],
        operations: vec![
            github_issue_list_operation(),
            github_issue_search_operation(),
        ],
        bindings: vec![
            github_issue_list_rest_binding(),
            github_issue_search_rest_binding(),
        ],
    }
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

pub fn github_issue_search_rest_binding() -> Binding {
    Binding {
        id: BindingId::new("github.issue.search.http"),
        operation: OperationId::new("github.issue.search"),
        surface: github_rest_surface().id,
        protocol: BindingProtocol::Http(HttpBinding {
            method: HttpMethod::Get,
            path: "/search/issues".to_string(),
            query: vec![
                QueryParamBinding {
                    name: "q".to_string(),
                    input: InputId::new("q"),
                },
                QueryParamBinding {
                    name: "sort".to_string(),
                    input: InputId::new("sort"),
                },
                QueryParamBinding {
                    name: "order".to_string(),
                    input: InputId::new("order"),
                },
            ],
            // GitHub search also returns total_count and incomplete_results.
            // This spike projects rows only; response-level metadata needs an
            // explicit model concept before it should be exposed to SQL.
            response: ResponseBinding {
                items_path: JsonPath("$.items".to_string()),
            },
            pagination: Some(Pagination::LinkHeader {
                page_size: PageSize {
                    query_param: "per_page".to_string(),
                    default: 30,
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

    #[test]
    fn source_model_can_be_created() {
        let gh_source_model = github_source_model();

        assert_eq!(gh_source_model.operations().len(), 2);
        assert_eq!(gh_source_model.operations()[0].kind(), &OperationKind::List);
        assert_eq!(gh_source_model.surfaces().len(), 1);
        assert_eq!(gh_source_model.surfaces()[0].kind(), &SurfaceKind::Rest);
    }

    #[test]
    fn github_issue_models_user_as_entity_field() {
        let issue = github_issue_entity();
        let user = issue
            .fields()
            .iter()
            .find(|field| field.name() == "user")
            .expect("issue user field");

        assert_eq!(user.ty(), &TypeRef::Entity(EntityId::new("github.user")));
        assert!(user.nullable());
    }

    #[test]
    fn github_operations_reference_issue_entity() {
        let model = github_source_model();
        let issue = EntityId::new("github.issue");

        assert_eq!(
            model
                .operations()
                .iter()
                .map(Operation::entity)
                .collect::<Vec<_>>(),
            vec![&issue, &issue]
        );
        assert!(model.entity(&issue).is_some());
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

    #[test]
    fn github_issue_search_rest_binding_matches_wrapped_response_shape() {
        let binding = github_issue_search_rest_binding();
        let http = binding.protocol().as_http();

        assert_eq!(binding.operation().as_str(), "github.issue.search");
        assert_eq!(binding.surface().as_str(), "github.rest");
        assert_eq!(http.method(), HttpMethod::Get);
        assert_eq!(http.path(), "/search/issues");
        assert_eq!(
            http.query()
                .iter()
                .map(|query| (query.name(), query.input().as_str()))
                .collect::<Vec<_>>(),
            vec![("q", "q"), ("sort", "sort"), ("order", "order")]
        );
        assert_eq!(http.response().items_path().as_str(), "$.items");

        let page_size = http
            .pagination()
            .expect("github issue search should expose a page-size parameter")
            .page_size();
        assert_eq!(page_size.query_param(), "per_page");
        assert_eq!(page_size.default(), 30);
        assert_eq!(page_size.max(), 100);
    }
}
