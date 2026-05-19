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
    Number,
    Boolean,
    Date,
    DateTime,
    Json,
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Scalar(ScalarType),
    Entity(EntityId),
    Enum(EnumId),
    List(Box<TypeRef>),
    Map(Box<TypeRef>),
    Union(Vec<TypeRef>),
}

impl TypeRef {
    pub fn list(inner: TypeRef) -> Self {
        Self::List(Box::new(inner))
    }

    pub fn map(value: TypeRef) -> Self {
        Self::Map(Box::new(value))
    }

    pub fn union(members: Vec<TypeRef>) -> Self {
        Self::Union(members)
    }
}

// ----- Operations ---------------------------------------

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
    inputs: Vec<OperationInput>,
    outcomes: Vec<OperationOutcome>,
}

impl Operation {
    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn primary_output(&self) -> Option<&TypeRef> {
        self.outcomes
            .iter()
            .find(|outcome| outcome.is_success())
            .and_then(OperationOutcome::body)
    }

    pub fn output(&self) -> &TypeRef {
        self.primary_output()
            .expect("operation should have a successful body output")
    }

    pub fn inputs(&self) -> &[OperationInput] {
        &self.inputs
    }

    pub fn outcomes(&self) -> &[OperationOutcome] {
        &self.outcomes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationOutcomeId(String);

impl OperationOutcomeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationOutcome {
    id: OperationOutcomeId,
    body: Option<TypeRef>,
    success: bool,
}

impl OperationOutcome {
    pub fn success(id: impl Into<String>, body: TypeRef) -> Self {
        Self {
            id: OperationOutcomeId::new(id),
            body: Some(body),
            success: true,
        }
    }

    pub fn success_no_body(id: impl Into<String>) -> Self {
        Self {
            id: OperationOutcomeId::new(id),
            body: None,
            success: true,
        }
    }

    pub fn error(id: impl Into<String>, body: Option<TypeRef>) -> Self {
        Self {
            id: OperationOutcomeId::new(id),
            body,
            success: false,
        }
    }

    pub fn id(&self) -> &OperationOutcomeId {
        &self.id
    }

    pub fn body(&self) -> Option<&TypeRef> {
        self.body.as_ref()
    }

    pub fn is_success(&self) -> bool {
        self.success
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
    required: bool,
    nullable: bool,
}

impl EntityField {
    pub fn new(name: impl Into<String>, ty: TypeRef, nullable: bool) -> Self {
        Self {
            name: name.into(),
            ty,
            required: true,
            nullable,
        }
    }

    pub fn optional(name: impl Into<String>, ty: TypeRef, nullable: bool) -> Self {
        Self {
            name: name.into(),
            ty,
            required: false,
            nullable,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> &TypeRef {
        &self.ty
    }

    pub fn required(&self) -> bool {
        self.required
    }

    pub fn nullable(&self) -> bool {
        self.nullable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    id: EntityId,
    role: EntityRole,
    fields: Vec<EntityField>,
}

impl Entity {
    pub fn new(id: EntityId, fields: Vec<EntityField>) -> Self {
        Self {
            id,
            role: EntityRole::Resource,
            fields,
        }
    }

    pub fn input(id: EntityId, fields: Vec<EntityField>) -> Self {
        Self {
            id,
            role: EntityRole::Input,
            fields,
        }
    }

    pub fn value(id: EntityId, fields: Vec<EntityField>) -> Self {
        Self {
            id,
            role: EntityRole::Value,
            fields,
        }
    }

    pub fn id(&self) -> &EntityId {
        &self.id
    }

    pub fn role(&self) -> EntityRole {
        self.role
    }

    pub fn fields(&self) -> &[EntityField] {
        &self.fields
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityRole {
    Resource,
    Input,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumId(String);

impl EnumId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumType {
    id: EnumId,
    values: Vec<String>,
}

impl EnumType {
    pub fn new(id: EnumId, values: Vec<String>) -> Self {
        Self { id, values }
    }

    pub fn id(&self) -> &EnumId {
        &self.id
    }

    pub fn values(&self) -> &[String] {
        &self.values
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
    parameters: Vec<HttpParameterBinding>,
    request_body: Option<HttpRequestBodyBinding>,
    responses: Vec<HttpResponseBinding>,
    pagination: Option<Pagination>,
}

impl HttpBinding {
    pub fn method(&self) -> HttpMethod {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn parameters(&self) -> &[HttpParameterBinding] {
        &self.parameters
    }

    pub fn query(&self) -> Vec<&HttpParameterBinding> {
        self.parameters
            .iter()
            .filter(|parameter| parameter.location() == HttpParameterLocation::Query)
            .collect()
    }

    pub fn request_body(&self) -> Option<&HttpRequestBodyBinding> {
        self.request_body.as_ref()
    }

    pub fn response(&self) -> &HttpResponseBinding {
        self.responses
            .iter()
            .find(|response| response.status().is_success())
            .expect("HTTP binding should define a success response")
    }

    pub fn responses(&self) -> &[HttpResponseBinding] {
        &self.responses
    }

    pub fn pagination(&self) -> Option<&Pagination> {
        self.pagination.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpParameterBinding {
    name: String,
    input: InputId,
    location: HttpParameterLocation,
    serialization: ParameterSerialization,
}

impl HttpParameterBinding {
    pub fn path(name: impl Into<String>, input: InputId) -> Self {
        Self::new(name, input, HttpParameterLocation::Path)
    }

    pub fn query(name: impl Into<String>, input: InputId) -> Self {
        Self::new(name, input, HttpParameterLocation::Query)
    }

    pub fn header(name: impl Into<String>, input: InputId) -> Self {
        Self::new(name, input, HttpParameterLocation::Header)
    }

    pub fn cookie(name: impl Into<String>, input: InputId) -> Self {
        Self::new(name, input, HttpParameterLocation::Cookie)
    }

    pub fn new(name: impl Into<String>, input: InputId, location: HttpParameterLocation) -> Self {
        Self {
            name: name.into(),
            input,
            location,
            serialization: ParameterSerialization::default(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn input(&self) -> &InputId {
        &self.input
    }

    pub fn location(&self) -> HttpParameterLocation {
        self.location
    }

    pub fn serialization(&self) -> &ParameterSerialization {
        &self.serialization
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterSerialization {
    style: ParameterStyle,
    explode: bool,
    allow_reserved: bool,
}

impl Default for ParameterSerialization {
    fn default() -> Self {
        Self {
            style: ParameterStyle::Form,
            explode: true,
            allow_reserved: false,
        }
    }
}

impl ParameterSerialization {
    pub fn style(&self) -> ParameterStyle {
        self.style
    }

    pub fn explode(&self) -> bool {
        self.explode
    }

    pub fn allow_reserved(&self) -> bool {
        self.allow_reserved
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterStyle {
    Matrix,
    Label,
    Form,
    Simple,
    SpaceDelimited,
    PipeDelimited,
    DeepObject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestBodyBinding {
    input: InputId,
    content_type: String,
    serialization: BodySerialization,
}

impl HttpRequestBodyBinding {
    pub fn json(input: InputId) -> Self {
        Self {
            input,
            content_type: "application/json".to_string(),
            serialization: BodySerialization::Json,
        }
    }

    pub fn input(&self) -> &InputId {
        &self.input
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    pub fn serialization(&self) -> BodySerialization {
        self.serialization
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodySerialization {
    Json,
    FormUrlEncoded,
    Text,
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponseBinding {
    outcome: OperationOutcomeId,
    status: HttpStatusPattern,
    content_type: Option<String>,
    output_path: JsonPath,
}

impl HttpResponseBinding {
    pub fn json_success(outcome: OperationOutcomeId, output_path: JsonPath) -> Self {
        Self {
            outcome,
            status: HttpStatusPattern::Success,
            content_type: Some("application/json".to_string()),
            output_path,
        }
    }

    pub fn no_body(outcome: OperationOutcomeId, status: u16) -> Self {
        Self {
            outcome,
            status: HttpStatusPattern::Exact(status),
            content_type: None,
            output_path: JsonPath("$".to_string()),
        }
    }

    pub fn outcome(&self) -> &OperationOutcomeId {
        &self.outcome
    }

    pub fn status(&self) -> HttpStatusPattern {
        self.status
    }

    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    pub fn output_path(&self) -> &JsonPath {
        &self.output_path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatusPattern {
    Exact(u16),
    Success,
    Default,
}

impl HttpStatusPattern {
    pub fn is_success(self) -> bool {
        match self {
            Self::Exact(status) => (200..300).contains(&status),
            Self::Success => true,
            Self::Default => false,
        }
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
    enums: Vec<EnumType>,
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

    pub fn enums(&self) -> &[EnumType] {
        &self.enums
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

    pub fn enum_type(&self, id: &EnumId) -> Option<&EnumType> {
        self.enums.iter().find(|enum_type| enum_type.id() == id)
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
            EntityField::new(
                "state",
                TypeRef::Enum(EnumId::new("github.issue_state")),
                false,
            ),
            EntityField::new("created_at", TypeRef::Scalar(ScalarType::DateTime), false),
            EntityField::new("html_url", TypeRef::Scalar(ScalarType::String), false),
            EntityField::new("user", TypeRef::Entity(EntityId::new("github.user")), true),
        ],
    )
}

pub fn github_issue_create_input_entity() -> Entity {
    Entity::input(
        EntityId::new("github.issue.create.input"),
        vec![
            EntityField::new("title", TypeRef::Scalar(ScalarType::String), false),
            EntityField::optional("body", TypeRef::Scalar(ScalarType::String), true),
            EntityField::optional(
                "assignees",
                TypeRef::list(TypeRef::Scalar(ScalarType::String)),
                false,
            ),
            EntityField::optional("milestone", TypeRef::Scalar(ScalarType::Integer), true),
            EntityField::optional(
                "labels",
                TypeRef::list(TypeRef::Scalar(ScalarType::String)),
                false,
            ),
        ],
    )
}

pub fn github_issue_state_enum() -> EnumType {
    EnumType::new(
        EnumId::new("github.issue_state"),
        vec!["open".to_string(), "closed".to_string()],
    )
}

pub fn github_issue_list_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.list"),
        inputs: vec![
            OperationInput::required("owner", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("repo", TypeRef::Scalar(ScalarType::String)),
            OperationInput::optional("state", TypeRef::Enum(EnumId::new("github.issue_state"))),
        ],
        outcomes: vec![OperationOutcome::success(
            "success",
            TypeRef::list(TypeRef::Entity(EntityId::new("github.issue"))),
        )],
    }
}

pub fn github_issue_search_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.search"),
        inputs: vec![
            OperationInput::required("q", TypeRef::Scalar(ScalarType::String)),
            OperationInput::optional("sort", TypeRef::Scalar(ScalarType::String)),
            OperationInput::optional("order", TypeRef::Scalar(ScalarType::String)),
        ],
        outcomes: vec![OperationOutcome::success(
            "success",
            TypeRef::list(TypeRef::Entity(EntityId::new("github.issue"))),
        )],
    }
}

pub fn github_issue_get_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.get"),
        inputs: vec![
            OperationInput::required("owner", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("repo", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("issue_number", TypeRef::Scalar(ScalarType::Integer)),
        ],
        outcomes: vec![OperationOutcome::success(
            "success",
            TypeRef::Entity(EntityId::new("github.issue")),
        )],
    }
}

pub fn github_issue_create_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.create"),
        inputs: vec![
            OperationInput::required("owner", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("repo", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required(
                "input",
                TypeRef::Entity(EntityId::new("github.issue.create.input")),
            ),
        ],
        outcomes: vec![OperationOutcome::success(
            "created",
            TypeRef::Entity(EntityId::new("github.issue")),
        )],
    }
}

pub fn github_issue_lock_operation() -> Operation {
    Operation {
        id: OperationId::new("github.issue.lock"),
        inputs: vec![
            OperationInput::required("owner", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("repo", TypeRef::Scalar(ScalarType::String)),
            OperationInput::required("issue_number", TypeRef::Scalar(ScalarType::Integer)),
        ],
        outcomes: vec![OperationOutcome::success_no_body("locked")],
    }
}

pub fn github_source_model() -> SourceModel {
    SourceModel {
        source: "github".to_string(),
        entities: vec![
            github_issue_entity(),
            github_user_entity(),
            github_issue_create_input_entity(),
        ],
        enums: vec![github_issue_state_enum()],
        surfaces: vec![github_rest_surface()],
        operations: vec![
            github_issue_list_operation(),
            github_issue_search_operation(),
            github_issue_get_operation(),
            github_issue_create_operation(),
            github_issue_lock_operation(),
        ],
        bindings: vec![
            github_issue_list_rest_binding(),
            github_issue_search_rest_binding(),
            github_issue_get_rest_binding(),
            github_issue_create_rest_binding(),
            github_issue_lock_rest_binding(),
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
            parameters: vec![
                HttpParameterBinding::path("owner", InputId::new("owner")),
                HttpParameterBinding::path("repo", InputId::new("repo")),
                HttpParameterBinding::query("state", InputId::new("state")),
            ],
            request_body: None,
            responses: vec![HttpResponseBinding::json_success(
                OperationOutcomeId::new("success"),
                JsonPath("$".to_string()),
            )],
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
            parameters: vec![
                HttpParameterBinding::query("q", InputId::new("q")),
                HttpParameterBinding::query("sort", InputId::new("sort")),
                HttpParameterBinding::query("order", InputId::new("order")),
            ],
            request_body: None,
            // GitHub search also returns total_count and incomplete_results.
            // This spike projects rows only; response-level metadata needs an
            // explicit model concept before it should be exposed to SQL.
            responses: vec![HttpResponseBinding::json_success(
                OperationOutcomeId::new("success"),
                JsonPath("$.items".to_string()),
            )],
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

pub fn github_issue_get_rest_binding() -> Binding {
    Binding {
        id: BindingId::new("github.issue.get.http"),
        operation: OperationId::new("github.issue.get"),
        surface: github_rest_surface().id,
        protocol: BindingProtocol::Http(HttpBinding {
            method: HttpMethod::Get,
            path: "/repos/{owner}/{repo}/issues/{issue_number}".to_string(),
            parameters: vec![
                HttpParameterBinding::path("owner", InputId::new("owner")),
                HttpParameterBinding::path("repo", InputId::new("repo")),
                HttpParameterBinding::path("issue_number", InputId::new("issue_number")),
            ],
            request_body: None,
            responses: vec![HttpResponseBinding::json_success(
                OperationOutcomeId::new("success"),
                JsonPath("$".to_string()),
            )],
            pagination: None,
        }),
    }
}

pub fn github_issue_create_rest_binding() -> Binding {
    Binding {
        id: BindingId::new("github.issue.create.http"),
        operation: OperationId::new("github.issue.create"),
        surface: github_rest_surface().id,
        protocol: BindingProtocol::Http(HttpBinding {
            method: HttpMethod::Post,
            path: "/repos/{owner}/{repo}/issues".to_string(),
            parameters: vec![
                HttpParameterBinding::path("owner", InputId::new("owner")),
                HttpParameterBinding::path("repo", InputId::new("repo")),
            ],
            request_body: Some(HttpRequestBodyBinding::json(InputId::new("input"))),
            responses: vec![HttpResponseBinding {
                outcome: OperationOutcomeId::new("created"),
                status: HttpStatusPattern::Exact(201),
                content_type: Some("application/json".to_string()),
                output_path: JsonPath("$".to_string()),
            }],
            pagination: None,
        }),
    }
}

pub fn github_issue_lock_rest_binding() -> Binding {
    Binding {
        id: BindingId::new("github.issue.lock.http"),
        operation: OperationId::new("github.issue.lock"),
        surface: github_rest_surface().id,
        protocol: BindingProtocol::Http(HttpBinding {
            method: HttpMethod::Put,
            path: "/repos/{owner}/{repo}/issues/{issue_number}/lock".to_string(),
            parameters: vec![
                HttpParameterBinding::path("owner", InputId::new("owner")),
                HttpParameterBinding::path("repo", InputId::new("repo")),
                HttpParameterBinding::path("issue_number", InputId::new("issue_number")),
            ],
            request_body: None,
            responses: vec![HttpResponseBinding::no_body(
                OperationOutcomeId::new("locked"),
                204,
            )],
            pagination: None,
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

        assert_eq!(gh_source_model.operations().len(), 5);
        assert_eq!(gh_source_model.enums().len(), 1);
        assert_eq!(gh_source_model.surfaces().len(), 1);
        assert_eq!(
            gh_source_model
                .surfaces()
                .first()
                .expect("first surface")
                .kind(),
            &SurfaceKind::Rest
        );
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
        assert!(user.required());
        assert!(user.nullable());
    }

    #[test]
    fn github_operations_express_issue_output_cardinality() {
        let model = github_source_model();
        let list = model
            .operation(&OperationId::new("github.issue.list"))
            .expect("list operation");
        let get = model
            .operation(&OperationId::new("github.issue.get"))
            .expect("get operation");

        assert_eq!(
            list.output(),
            &TypeRef::list(TypeRef::Entity(EntityId::new("github.issue")))
        );
        assert_eq!(
            get.output(),
            &TypeRef::Entity(EntityId::new("github.issue"))
        );
        assert!(model.entity(&EntityId::new("github.issue")).is_some());
    }

    #[test]
    fn github_create_issue_models_body_as_logical_input_entity() {
        let create = github_issue_create_operation();
        let input = create
            .inputs()
            .iter()
            .find(|input| input.name() == "input")
            .expect("create issue body input");

        assert_eq!(
            input.ty(),
            &TypeRef::Entity(EntityId::new("github.issue.create.input"))
        );
        assert!(input.is_required());

        let input_entity = github_issue_create_input_entity();
        assert_eq!(input_entity.role(), EntityRole::Input);
        let body = input_entity
            .fields()
            .iter()
            .find(|field| field.name() == "body")
            .expect("optional body field");
        assert!(!body.required());
        assert!(body.nullable());
    }

    #[test]
    fn github_lock_issue_models_no_body_success_outcome() {
        let lock = github_issue_lock_operation();
        let outcome = lock.outcomes().first().expect("lock outcome");

        assert_eq!(outcome.id().as_str(), "locked");
        assert!(outcome.is_success());
        assert!(outcome.body().is_none());
        assert!(lock.primary_output().is_none());
    }

    #[test]
    fn github_issues_rest_binding_matches_spike_request_shape() {
        let binding = github_issue_list_rest_binding();
        let http = binding.protocol().as_http();

        assert_eq!(binding.operation().as_str(), "github.issue.list");
        assert_eq!(binding.surface().as_str(), "github.rest");
        assert_eq!(http.method(), HttpMethod::Get);
        assert_eq!(http.path(), "/repos/{owner}/{repo}/issues");
        assert_eq!(
            http.parameters()
                .iter()
                .map(|parameter| (parameter.name(), parameter.location()))
                .collect::<Vec<_>>(),
            vec![
                ("owner", HttpParameterLocation::Path),
                ("repo", HttpParameterLocation::Path),
                ("state", HttpParameterLocation::Query),
            ]
        );
        let queries = http.query();
        let state_query = queries.first().expect("state query");
        assert_eq!(state_query.name(), "state");
        assert_eq!(state_query.input().as_str(), "state");

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
        assert_eq!(http.response().output_path().as_str(), "$.items");
        assert_eq!(http.response().outcome().as_str(), "success");

        let page_size = http
            .pagination()
            .expect("github issue search should expose a page-size parameter")
            .page_size();
        assert_eq!(page_size.query_param(), "per_page");
        assert_eq!(page_size.default(), 30);
        assert_eq!(page_size.max(), 100);
    }

    #[test]
    fn github_issue_get_rest_binding_matches_singleton_request_shape() {
        let binding = github_issue_get_rest_binding();
        let http = binding.protocol().as_http();

        assert_eq!(binding.operation().as_str(), "github.issue.get");
        assert_eq!(binding.surface().as_str(), "github.rest");
        assert_eq!(http.method(), HttpMethod::Get);
        assert_eq!(http.path(), "/repos/{owner}/{repo}/issues/{issue_number}");
        assert!(http.query().is_empty());
        assert_eq!(http.response().output_path().as_str(), "$");
        assert!(http.pagination().is_none());
    }

    #[test]
    fn github_issue_create_rest_binding_maps_body_at_binding_layer() {
        let binding = github_issue_create_rest_binding();
        let http = binding.protocol().as_http();

        assert_eq!(binding.operation().as_str(), "github.issue.create");
        assert_eq!(http.method(), HttpMethod::Post);
        assert_eq!(
            http.parameters()
                .iter()
                .map(|parameter| (
                    parameter.name(),
                    parameter.input().as_str(),
                    parameter.location()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("owner", "owner", HttpParameterLocation::Path),
                ("repo", "repo", HttpParameterLocation::Path),
            ]
        );
        let body = http.request_body().expect("create issue body binding");
        assert_eq!(body.input().as_str(), "input");
        assert_eq!(body.content_type(), "application/json");
        assert_eq!(body.serialization(), BodySerialization::Json);
        assert_eq!(http.response().status(), HttpStatusPattern::Exact(201));
        assert_eq!(http.response().outcome().as_str(), "created");
    }

    #[test]
    fn github_issue_lock_rest_binding_maps_no_body_response() {
        let binding = github_issue_lock_rest_binding();
        let http = binding.protocol().as_http();

        assert_eq!(binding.operation().as_str(), "github.issue.lock");
        assert_eq!(http.method(), HttpMethod::Put);
        assert!(http.request_body().is_none());
        assert_eq!(http.response().status(), HttpStatusPattern::Exact(204));
        assert_eq!(http.response().outcome().as_str(), "locked");
        assert!(http.response().content_type().is_none());
    }
}
