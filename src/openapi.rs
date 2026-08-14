use std::collections::BTreeMap;

use crate::{IntoResponse, Response, SchemaField, SchemaKind, SchemaMetadata};

#[derive(Debug, Clone)]
pub struct OpenApi {
    title: String,
    version: String,
    servers: Vec<Server>,
    security_schemes: BTreeMap<String, SecurityScheme>,
    security: Vec<SecurityRequirement>,
    scalar: Scalar,
}

impl OpenApi {
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            servers: Vec::new(),
            security_schemes: BTreeMap::new(),
            security: Vec::new(),
            scalar: Scalar::new(),
        }
    }

    pub fn server(mut self, server: Server) -> Self {
        self.servers.push(server);
        self
    }

    pub fn security_scheme(mut self, name: impl Into<String>, scheme: SecurityScheme) -> Self {
        self.security_schemes.insert(name.into(), scheme);
        self
    }

    pub fn security(mut self, requirement: SecurityRequirement) -> Self {
        self.security.push(requirement);
        self
    }

    pub fn scalar_config(mut self, scalar: Scalar) -> Self {
        self.scalar = scalar;
        self
    }

    pub(crate) fn scalar_page(&self, document: &OpenApiDocument) -> String {
        render_scalar_page(&self.title, document.as_str(), &self.scalar)
    }

    pub(crate) fn build(&self, routes: Vec<RouteDescription>) -> OpenApiDocument {
        self.validate(&routes);
        OpenApiDocument {
            json: render_document(self, routes),
        }
    }

    fn validate(&self, routes: &[RouteDescription]) {
        for name in self.security_schemes.keys() {
            assert!(
                !name.is_empty(),
                "OpenAPI security scheme names cannot be empty"
            );
        }
        validate_security_requirements(&self.security, &self.security_schemes);

        for scheme in self.security_schemes.values() {
            if let SecurityScheme::OAuth2(flows) = scheme {
                validate_oauth_flows(flows);
            }
        }

        let mut operation_ids = Vec::<&str>::new();
        for route in routes {
            validate_security_requirements(&route.operation.security, &self.security_schemes);
            if let Some(operation_id) = route.operation.operation_id.as_deref() {
                assert!(
                    !operation_id.is_empty(),
                    "OpenAPI operation IDs cannot be empty",
                );
                assert!(
                    !operation_ids.contains(&operation_id),
                    "duplicate OpenAPI operation ID `{operation_id}`",
                );
                operation_ids.push(operation_id);
            }
        }
    }
}

fn validate_security_requirements(
    requirements: &[SecurityRequirement],
    schemes: &BTreeMap<String, SecurityScheme>,
) {
    for requirement in requirements {
        assert!(
            schemes.contains_key(&requirement.scheme),
            "OpenAPI security requirement references unknown scheme `{}`",
            requirement.scheme,
        );
    }
}

fn validate_oauth_flows(flows: &OAuthFlows) {
    assert!(
        flows.implicit.is_some()
            || flows.password.is_some()
            || flows.client_credentials.is_some()
            || flows.authorization_code.is_some(),
        "an OAuth2 security scheme requires at least one flow",
    );

    if let Some(flow) = &flows.implicit {
        assert!(
            flow.authorization_url.is_some(),
            "an implicit OAuth2 flow requires an authorization URL",
        );
    }
    if let Some(flow) = &flows.password {
        assert!(
            flow.token_url.is_some(),
            "a password OAuth2 flow requires a token URL",
        );
    }
    if let Some(flow) = &flows.client_credentials {
        assert!(
            flow.token_url.is_some(),
            "a client credentials OAuth2 flow requires a token URL",
        );
    }
    if let Some(flow) = &flows.authorization_code {
        assert!(
            flow.authorization_url.is_some() && flow.token_url.is_some(),
            "an authorization code OAuth2 flow requires authorization and token URLs",
        );
    }
}

#[derive(Debug, Clone)]
pub struct Server {
    url: String,
    description: Option<String>,
}

impl Server {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            description: None,
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

impl ApiKeyLocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::Query => "query",
            Self::Cookie => "cookie",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SecurityScheme {
    ApiKey {
        name: String,
        location: ApiKeyLocation,
    },
    Http {
        scheme: String,
        bearer_format: Option<String>,
    },
    OpenIdConnect {
        url: String,
    },
    OAuth2(Box<OAuthFlows>),
}

impl SecurityScheme {
    pub fn api_key(name: impl Into<String>, location: ApiKeyLocation) -> Self {
        Self::ApiKey {
            name: name.into(),
            location,
        }
    }

    pub fn http(scheme: impl Into<String>) -> Self {
        Self::Http {
            scheme: scheme.into(),
            bearer_format: None,
        }
    }

    pub fn bearer() -> Self {
        Self::http("bearer")
    }

    pub fn bearer_format(mut self, format: impl Into<String>) -> Self {
        if let Self::Http { bearer_format, .. } = &mut self {
            *bearer_format = Some(format.into());
        }
        self
    }

    pub fn open_id_connect(url: impl Into<String>) -> Self {
        Self::OpenIdConnect { url: url.into() }
    }

    pub fn oauth2(flows: OAuthFlows) -> Self {
        Self::OAuth2(Box::new(flows))
    }
}

#[derive(Debug, Clone, Default)]
pub struct OAuthFlows {
    implicit: Option<OAuthFlow>,
    password: Option<OAuthFlow>,
    client_credentials: Option<OAuthFlow>,
    authorization_code: Option<OAuthFlow>,
}

impl OAuthFlows {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn implicit(mut self, flow: OAuthFlow) -> Self {
        self.implicit = Some(flow);
        self
    }

    pub fn password(mut self, flow: OAuthFlow) -> Self {
        self.password = Some(flow);
        self
    }

    pub fn client_credentials(mut self, flow: OAuthFlow) -> Self {
        self.client_credentials = Some(flow);
        self
    }

    pub fn authorization_code(mut self, flow: OAuthFlow) -> Self {
        self.authorization_code = Some(flow);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct OAuthFlow {
    authorization_url: Option<String>,
    token_url: Option<String>,
    refresh_url: Option<String>,
    scopes: BTreeMap<String, String>,
}

impl OAuthFlow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn authorization_url(mut self, url: impl Into<String>) -> Self {
        self.authorization_url = Some(url.into());
        self
    }

    pub fn token_url(mut self, url: impl Into<String>) -> Self {
        self.token_url = Some(url.into());
        self
    }

    pub fn refresh_url(mut self, url: impl Into<String>) -> Self {
        self.refresh_url = Some(url.into());
        self
    }

    pub fn scope(mut self, name: impl Into<String>, description: impl Into<String>) -> Self {
        self.scopes.insert(name.into(), description.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct SecurityRequirement {
    scheme: String,
    scopes: Vec<String>,
}

impl SecurityRequirement {
    pub fn new(scheme: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            scopes: Vec::new(),
        }
    }

    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scopes.push(scope.into());
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ScalarDeveloperTools {
    Always,
    Localhost,
    Never,
}

impl ScalarDeveloperTools {
    fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Localhost => "localhost",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scalar {
    theme: Option<String>,
    show_sidebar: Option<bool>,
    developer_tools: Option<ScalarDeveloperTools>,
    default_fonts: Option<bool>,
}

impl Scalar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn theme(mut self, theme: impl Into<String>) -> Self {
        self.theme = Some(theme.into());
        self
    }

    pub fn show_sidebar(mut self, show: bool) -> Self {
        self.show_sidebar = Some(show);
        self
    }

    pub fn developer_tools(mut self, mode: ScalarDeveloperTools) -> Self {
        self.developer_tools = Some(mode);
        self
    }

    pub fn default_fonts(mut self, enabled: bool) -> Self {
        self.default_fonts = Some(enabled);
        self
    }
}

fn render_scalar_page(title: &str, document: &str, configuration: &Scalar) -> String {
    let mut output = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>",
    );
    html_text(&mut output, title);
    output.push_str(
        "</title></head><body><div id=\"app\"></div><script src=\"https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.63.0\"></script><script>Scalar.createApiReference('#app',{content:",
    );
    script_string(&mut output, document);
    if let Some(theme) = &configuration.theme {
        output.push_str(",theme:");
        script_string(&mut output, theme);
    }
    if let Some(show) = configuration.show_sidebar {
        output.push_str(",showSidebar:");
        output.push_str(if show { "true" } else { "false" });
    }
    if let Some(mode) = configuration.developer_tools {
        output.push_str(",showDeveloperTools:");
        script_string(&mut output, mode.as_str());
    }
    if let Some(enabled) = configuration.default_fonts {
        output.push_str(",withDefaultFonts:");
        output.push_str(if enabled { "true" } else { "false" });
    }
    output.push_str("})</script></body></html>");
    output
}

fn html_text(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            character => output.push(character),
        }
    }
}

fn script_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '<' => output.push_str("\\u003c"),
            '>' => output.push_str("\\u003e"),
            '&' => output.push_str("\\u0026"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            character if character < '\u{20}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[derive(Debug, Clone)]
pub struct OpenApiDocument {
    json: String,
}

impl OpenApiDocument {
    pub fn as_str(&self) -> &str {
        &self.json
    }

    pub fn into_string(self) -> String {
        self.json
    }
}

impl IntoResponse for OpenApiDocument {
    fn into_response(self) -> Response {
        let mut response = Response::bytes(200, self.json.into_bytes());
        response.set_header("Content-Type", "application/json");
        response
    }

    fn openapi(operation: &mut Operation) {
        operation.response(200, "OpenAPI document", Some("application/json"), None);
    }
}

#[derive(Debug, Default, Clone)]
pub struct Operation {
    summary: Option<String>,
    description: Option<String>,
    operation_id: Option<String>,
    tags: Vec<String>,
    security: Vec<SecurityRequirement>,
    parameter_groups: Vec<ParameterGroup>,
    request_body: Option<RequestBody>,
    responses: Vec<ApiResponse>,
}

impl Operation {
    pub fn summary(&mut self, summary: impl Into<String>) -> &mut Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn description(&mut self, description: impl Into<String>) -> &mut Self {
        self.description = Some(description.into());
        self
    }

    pub fn operation_id(&mut self, operation_id: impl Into<String>) -> &mut Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn tag(&mut self, tag: impl Into<String>) -> &mut Self {
        self.tags.push(tag.into());
        self
    }

    pub fn security(&mut self, requirement: SecurityRequirement) -> &mut Self {
        self.security.push(requirement);
        self
    }

    pub fn parameter(&mut self, location: ParameterLocation, schema: SchemaMetadata) -> &mut Self {
        self.parameter_groups
            .push(ParameterGroup { location, schema });
        self
    }

    pub fn request_body(
        &mut self,
        content_type: &str,
        schema: Option<SchemaMetadata>,
        required: bool,
    ) -> &mut Self {
        let request_body = self.request_body.get_or_insert_with(|| RequestBody {
            required,
            content: Vec::new(),
        });
        request_body.required |= required;
        if let Some(media) = request_body
            .content
            .iter_mut()
            .find(|media| media.content_type == content_type)
        {
            media.schema = schema;
        } else {
            request_body.content.push(MediaType {
                content_type: content_type.to_owned(),
                schema,
                examples: Vec::new(),
            });
        }
        self
    }

    pub fn request_example(
        &mut self,
        content_type: &str,
        name: impl Into<String>,
        value: impl Into<ExampleValue>,
    ) -> &mut Self {
        let body = self.request_body.get_or_insert_with(|| RequestBody {
            required: false,
            content: Vec::new(),
        });
        let media = media_entry(&mut body.content, content_type);
        set_example(&mut media.examples, name.into(), value.into());
        self
    }

    pub fn response(
        &mut self,
        status: u16,
        description: &str,
        content_type: Option<&str>,
        schema: Option<SchemaMetadata>,
    ) -> &mut Self {
        let response = if let Some(response) = self
            .responses
            .iter_mut()
            .find(|response| response.status == status)
        {
            response.description = description.to_owned();
            response
        } else {
            self.responses.push(ApiResponse {
                status,
                description: description.to_owned(),
                content: Vec::new(),
                headers: Vec::new(),
            });
            self.responses.last_mut().unwrap()
        };

        if let Some(content_type) = content_type {
            media_entry(&mut response.content, content_type).schema = schema;
        }
        self
    }

    pub fn response_header(
        &mut self,
        status: u16,
        name: impl Into<String>,
        description: impl Into<String>,
        schema: SchemaMetadata,
    ) -> &mut Self {
        let response = self.response_entry(status);
        let name = name.into();
        if let Some(header) = response
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(&name))
        {
            header.description = description.into();
            header.schema = schema;
        } else {
            response.headers.push(ResponseHeader {
                name,
                description: description.into(),
                schema,
            });
        }
        self
    }

    pub fn response_example(
        &mut self,
        status: u16,
        content_type: &str,
        name: impl Into<String>,
        value: impl Into<ExampleValue>,
    ) -> &mut Self {
        let response = self.response_entry(status);
        let media = media_entry(&mut response.content, content_type);
        set_example(&mut media.examples, name.into(), value.into());
        self
    }

    pub(crate) fn ensure_response(&mut self) {
        if self.responses.is_empty() {
            self.response(200, "Success", None, None);
        }
    }

    fn response_entry(&mut self, status: u16) -> &mut ApiResponse {
        if let Some(index) = self
            .responses
            .iter()
            .position(|response| response.status == status)
        {
            return &mut self.responses[index];
        }

        self.responses.push(ApiResponse {
            status,
            description: "Response".to_owned(),
            content: Vec::new(),
            headers: Vec::new(),
        });
        self.responses.last_mut().unwrap()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExampleValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Unsigned(u64),
    Number(f64),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

impl ExampleValue {
    pub fn array(values: impl IntoIterator<Item = impl Into<Self>>) -> Self {
        Self::Array(values.into_iter().map(Into::into).collect())
    }

    pub fn object(entries: impl IntoIterator<Item = (impl Into<String>, impl Into<Self>)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
    }
}

impl From<&str> for ExampleValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for ExampleValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for ExampleValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

macro_rules! example_integer {
    ($($type:ty),+ $(,)?) => {
        $(
            impl From<$type> for ExampleValue {
                fn from(value: $type) -> Self {
                    Self::Integer(value.into())
                }
            }
        )+
    };
}

example_integer!(i8, i16, i32, i64);

macro_rules! example_unsigned {
    ($($type:ty),+ $(,)?) => {
        $(
            impl From<$type> for ExampleValue {
                fn from(value: $type) -> Self {
                    Self::Unsigned(value.into())
                }
            }
        )+
    };
}

example_unsigned!(u8, u16, u32, u64);

impl From<f32> for ExampleValue {
    fn from(value: f32) -> Self {
        Self::Number(value.into())
    }
}

impl From<f64> for ExampleValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

impl From<Vec<ExampleValue>> for ExampleValue {
    fn from(value: Vec<ExampleValue>) -> Self {
        Self::Array(value)
    }
}

impl From<BTreeMap<String, ExampleValue>> for ExampleValue {
    fn from(value: BTreeMap<String, ExampleValue>) -> Self {
        Self::Object(value)
    }
}

#[cfg(feature = "json")]
impl From<serde_json::Value> for ExampleValue {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Boolean(value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Self::Integer(value)
                } else if let Some(value) = value.as_u64() {
                    Self::Unsigned(value)
                } else {
                    Self::Number(
                        value
                            .as_f64()
                            .expect("a serde_json number can be represented as f64"),
                    )
                }
            }
            serde_json::Value::String(value) => Self::String(value),
            serde_json::Value::Array(values) => {
                Self::Array(values.into_iter().map(Self::from).collect())
            }
            serde_json::Value::Object(entries) => Self::Object(
                entries
                    .into_iter()
                    .map(|(name, value)| (name, Self::from(value)))
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct ParameterGroup {
    location: ParameterLocation,
    schema: SchemaMetadata,
}

#[derive(Debug, Clone)]
struct RequestBody {
    required: bool,
    content: Vec<MediaType>,
}

#[derive(Debug, Clone)]
struct MediaType {
    content_type: String,
    schema: Option<SchemaMetadata>,
    examples: Vec<Example>,
}

#[derive(Debug, Clone)]
struct ApiResponse {
    status: u16,
    description: String,
    content: Vec<MediaType>,
    headers: Vec<ResponseHeader>,
}

#[derive(Debug, Clone)]
struct ResponseHeader {
    name: String,
    description: String,
    schema: SchemaMetadata,
}

#[derive(Debug, Clone)]
struct Example {
    name: String,
    value: ExampleValue,
}

fn media_entry<'media>(
    content: &'media mut Vec<MediaType>,
    content_type: &str,
) -> &'media mut MediaType {
    if let Some(index) = content
        .iter()
        .position(|media| media.content_type == content_type)
    {
        return &mut content[index];
    }

    content.push(MediaType {
        content_type: content_type.to_owned(),
        schema: None,
        examples: Vec::new(),
    });
    content.last_mut().unwrap()
}

fn set_example(examples: &mut Vec<Example>, name: String, value: ExampleValue) {
    if let Some(example) = examples.iter_mut().find(|example| example.name == name) {
        example.value = value;
    } else {
        examples.push(Example { name, value });
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RouteDescription {
    pub path: String,
    pub method: String,
    pub path_parameters: Vec<String>,
    pub operation: Operation,
}

fn render_document(configuration: &OpenApi, routes: Vec<RouteDescription>) -> String {
    let components = collect_components(&routes);
    let mut paths = BTreeMap::<String, Vec<RouteDescription>>::new();

    for route in routes {
        paths.entry(route.path.clone()).or_default().push(route);
    }

    let mut output = String::from("{\"openapi\":\"3.1.0\",\"info\":{");
    property(&mut output, "title", &configuration.title);
    output.push(',');
    property(&mut output, "version", &configuration.version);
    output.push('}');

    if !configuration.servers.is_empty() {
        output.push_str(",\"servers\":[");
        for (index, server) in configuration.servers.iter().enumerate() {
            comma(&mut output, index);
            output.push('{');
            property(&mut output, "url", &server.url);
            if let Some(description) = &server.description {
                output.push(',');
                property(&mut output, "description", description);
            }
            output.push('}');
        }
        output.push(']');
    }

    if !configuration.security.is_empty() {
        output.push_str(",\"security\":");
        render_security_requirements(&mut output, &configuration.security);
    }

    output.push_str(",\"paths\":{");

    for (path_index, (path, routes)) in paths.into_iter().enumerate() {
        comma(&mut output, path_index);
        string(&mut output, &path);
        output.push_str(":{");

        for (method_index, route) in routes.into_iter().enumerate() {
            comma(&mut output, method_index);
            string(&mut output, &route.method.to_ascii_lowercase());
            output.push_str(":{");
            render_operation(&mut output, route);
            output.push('}');
        }

        output.push('}');
    }

    output.push('}');

    if !components.is_empty() || !configuration.security_schemes.is_empty() {
        output.push_str(",\"components\":{");
        let mut section = 0;

        if !components.is_empty() {
            comma(&mut output, section);
            section += 1;
            output.push_str("\"schemas\":{");
            for (index, (name, schema)) in components.iter().enumerate() {
                comma(&mut output, index);
                string(&mut output, name);
                output.push(':');
                render_schema_inline(&mut output, schema);
            }
            output.push('}');
        }

        if !configuration.security_schemes.is_empty() {
            comma(&mut output, section);
            output.push_str("\"securitySchemes\":{");
            for (index, (name, scheme)) in configuration.security_schemes.iter().enumerate() {
                comma(&mut output, index);
                string(&mut output, name);
                output.push(':');
                render_security_scheme(&mut output, scheme);
            }
            output.push('}');
        }

        output.push('}');
    }

    output.push('}');
    output
}

fn render_operation(output: &mut String, route: RouteDescription) {
    let parameters = expand_parameters(&route.operation, &route.path_parameters);
    let operation = route.operation;
    let mut field = 0;

    if let Some(summary) = &operation.summary {
        comma(output, field);
        field += 1;
        property(output, "summary", summary);
    }
    if let Some(description) = &operation.description {
        comma(output, field);
        field += 1;
        property(output, "description", description);
    }
    if let Some(operation_id) = &operation.operation_id {
        comma(output, field);
        field += 1;
        property(output, "operationId", operation_id);
    }
    if !operation.tags.is_empty() {
        comma(output, field);
        field += 1;
        output.push_str("\"tags\":[");
        for (index, tag) in operation.tags.iter().enumerate() {
            comma(output, index);
            string(output, tag);
        }
        output.push(']');
    }
    if !operation.security.is_empty() {
        comma(output, field);
        field += 1;
        output.push_str("\"security\":");
        render_security_requirements(output, &operation.security);
    }

    comma(output, field);
    output.push_str("\"parameters\":[");

    for (index, parameter) in parameters.iter().enumerate() {
        comma(output, index);
        output.push('{');
        property(output, "name", &parameter.name);
        output.push_str(",\"in\":");
        string(output, parameter.location.as_str());
        output.push_str(",\"required\":");
        output.push_str(if parameter.required { "true" } else { "false" });
        output.push_str(",\"schema\":");
        if let Some(field) = &parameter.field {
            render_field_schema(output, field);
        } else {
            render_schema(output, &parameter.schema);
        }
        if parameter.indexed {
            output.push_str(",\"x-serverkit-indexed\":true");
        }
        output.push('}');
    }

    output.push(']');

    if let Some(request_body) = &operation.request_body {
        output.push_str(",\"requestBody\":{\"required\":");
        output.push_str(if request_body.required {
            "true"
        } else {
            "false"
        });
        output.push_str(",\"content\":{");

        for (index, media_type) in request_body.content.iter().enumerate() {
            comma(output, index);
            string(output, &media_type.content_type);
            output.push(':');
            render_media_type(output, media_type);
        }

        output.push_str("}}");
    }

    output.push_str(",\"responses\":{");
    let mut responses = operation.responses;
    responses.sort_by_key(|response| response.status);

    for (index, response) in responses.iter().enumerate() {
        comma(output, index);
        string(output, &response.status.to_string());
        output.push_str(":{");
        property(output, "description", &response.description);

        if !response.headers.is_empty() {
            output.push_str(",\"headers\":{");
            for (header_index, header) in response.headers.iter().enumerate() {
                comma(output, header_index);
                string(output, &header.name);
                output.push_str(":{");
                property(output, "description", &header.description);
                output.push_str(",\"schema\":");
                render_schema(output, &header.schema);
                output.push('}');
            }
            output.push('}');
        }

        if !response.content.is_empty() {
            output.push_str(",\"content\":{");
            for (media_index, media) in response.content.iter().enumerate() {
                comma(output, media_index);
                string(output, &media.content_type);
                output.push(':');
                render_media_type(output, media);
            }
            output.push('}');
        }

        output.push('}');
    }

    output.push('}');
}

fn render_media_type(output: &mut String, media: &MediaType) {
    output.push('{');
    let mut field = 0;

    if let Some(schema) = &media.schema {
        output.push_str("\"schema\":");
        render_schema(output, schema);
        field += 1;
    }

    if !media.examples.is_empty() {
        comma(output, field);
        output.push_str("\"examples\":{");
        for (index, example) in media.examples.iter().enumerate() {
            comma(output, index);
            string(output, &example.name);
            output.push_str(":{\"value\":");
            render_example_value(output, &example.value);
            output.push('}');
        }
        output.push('}');
    }

    output.push('}');
}

fn render_security_requirements(output: &mut String, requirements: &[SecurityRequirement]) {
    output.push('[');
    for (index, requirement) in requirements.iter().enumerate() {
        comma(output, index);
        output.push('{');
        string(output, &requirement.scheme);
        output.push_str(":[");
        for (scope_index, scope) in requirement.scopes.iter().enumerate() {
            comma(output, scope_index);
            string(output, scope);
        }
        output.push_str("]}");
    }
    output.push(']');
}

fn render_example_value(output: &mut String, value: &ExampleValue) {
    match value {
        ExampleValue::Null => output.push_str("null"),
        ExampleValue::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        ExampleValue::Integer(value) => output.push_str(&value.to_string()),
        ExampleValue::Unsigned(value) => output.push_str(&value.to_string()),
        ExampleValue::Number(value) => {
            assert!(
                value.is_finite(),
                "OpenAPI example numbers must be finite JSON numbers",
            );
            output.push_str(&value.to_string());
        }
        ExampleValue::String(value) => string(output, value),
        ExampleValue::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                comma(output, index);
                render_example_value(output, value);
            }
            output.push(']');
        }
        ExampleValue::Object(entries) => {
            output.push('{');
            for (index, (name, value)) in entries.iter().enumerate() {
                comma(output, index);
                string(output, name);
                output.push(':');
                render_example_value(output, value);
            }
            output.push('}');
        }
    }
}

fn render_security_scheme(output: &mut String, scheme: &SecurityScheme) {
    output.push('{');
    match scheme {
        SecurityScheme::ApiKey { name, location } => {
            property(output, "type", "apiKey");
            output.push(',');
            property(output, "name", name);
            output.push(',');
            property(output, "in", location.as_str());
        }
        SecurityScheme::Http {
            scheme,
            bearer_format,
        } => {
            property(output, "type", "http");
            output.push(',');
            property(output, "scheme", scheme);
            if let Some(format) = bearer_format {
                output.push(',');
                property(output, "bearerFormat", format);
            }
        }
        SecurityScheme::OpenIdConnect { url } => {
            property(output, "type", "openIdConnect");
            output.push(',');
            property(output, "openIdConnectUrl", url);
        }
        SecurityScheme::OAuth2(flows) => {
            property(output, "type", "oauth2");
            output.push_str(",\"flows\":{");
            let entries = [
                ("implicit", flows.implicit.as_ref()),
                ("password", flows.password.as_ref()),
                ("clientCredentials", flows.client_credentials.as_ref()),
                ("authorizationCode", flows.authorization_code.as_ref()),
            ];
            let mut index = 0;
            for (name, flow) in entries {
                let Some(flow) = flow else { continue };
                comma(output, index);
                index += 1;
                string(output, name);
                output.push(':');
                render_oauth_flow(output, flow);
            }
            output.push('}');
        }
    }
    output.push('}');
}

fn render_oauth_flow(output: &mut String, flow: &OAuthFlow) {
    output.push('{');
    let mut field = 0;
    for (name, value) in [
        ("authorizationUrl", flow.authorization_url.as_ref()),
        ("tokenUrl", flow.token_url.as_ref()),
        ("refreshUrl", flow.refresh_url.as_ref()),
    ] {
        let Some(value) = value else { continue };
        comma(output, field);
        field += 1;
        property(output, name, value);
    }
    comma(output, field);
    output.push_str("\"scopes\":{");
    for (index, (name, description)) in flow.scopes.iter().enumerate() {
        comma(output, index);
        property(output, name, description);
    }
    output.push_str("}}");
}

fn collect_components(routes: &[RouteDescription]) -> BTreeMap<String, SchemaMetadata> {
    let mut components = BTreeMap::new();
    for route in routes {
        for group in &route.operation.parameter_groups {
            collect_schema(&group.schema, &mut components);
        }
        if let Some(body) = &route.operation.request_body {
            for media in &body.content {
                if let Some(schema) = &media.schema {
                    collect_schema(schema, &mut components);
                }
            }
        }
        for response in &route.operation.responses {
            for media in &response.content {
                if let Some(schema) = &media.schema {
                    collect_schema(schema, &mut components);
                }
            }
            for header in &response.headers {
                collect_schema(&header.schema, &mut components);
            }
        }
    }
    components
}

fn collect_schema(schema: &SchemaMetadata, components: &mut BTreeMap<String, SchemaMetadata>) {
    if let Some(name) = schema.name() {
        components
            .entry(name.to_owned())
            .or_insert_with(|| schema.clone());
    }

    match schema.kind() {
        SchemaKind::Object(fields) => {
            for field in fields {
                collect_schema(field.schema(), components);
            }
        }
        SchemaKind::Array(items) => collect_schema(items, components),
        SchemaKind::OneOf(schemas) => {
            for schema in schemas {
                collect_schema(schema, components);
            }
        }
        _ => {}
    }
}

struct ExpandedParameter {
    name: String,
    location: ParameterLocation,
    required: bool,
    schema: SchemaMetadata,
    field: Option<SchemaField>,
    indexed: bool,
}

fn expand_parameters(operation: &Operation, path_parameters: &[String]) -> Vec<ExpandedParameter> {
    let mut parameters = Vec::new();

    for group in &operation.parameter_groups {
        match group.schema.kind() {
            SchemaKind::Object(_) | SchemaKind::OneOf(_) => flatten_parameter_schema(
                &mut parameters,
                group.location,
                "",
                &group.schema,
                true,
                false,
                None,
            ),
            _ if group.location == ParameterLocation::Path && path_parameters.len() == 1 => {
                parameters.push(ExpandedParameter {
                    name: path_parameters[0].clone(),
                    location: group.location,
                    required: true,
                    schema: group.schema.clone(),
                    field: None,
                    indexed: false,
                });
            }
            _ => {}
        }
    }

    parameters
}

fn flatten_fields(
    output: &mut Vec<ExpandedParameter>,
    location: ParameterLocation,
    prefix: &str,
    fields: &[SchemaField],
    parent_required: bool,
    indexed: bool,
) {
    for field in fields {
        let name = if prefix.is_empty() {
            field.name().to_owned()
        } else {
            format!("{prefix}.{}", field.name())
        };

        flatten_parameter_schema(
            output,
            location,
            &name,
            field.schema(),
            parent_required && field.required(),
            indexed,
            Some(field),
        );
    }
}

fn flatten_parameter_schema(
    output: &mut Vec<ExpandedParameter>,
    location: ParameterLocation,
    name: &str,
    schema: &SchemaMetadata,
    required: bool,
    indexed: bool,
    field: Option<&SchemaField>,
) {
    match schema.kind() {
        SchemaKind::Object(fields) => {
            flatten_fields(output, location, name, fields, required, indexed);
        }
        SchemaKind::OneOf(variants) => {
            flatten_one_of(output, location, name, variants, required, indexed);
        }
        SchemaKind::Array(items)
            if matches!(items.kind(), SchemaKind::Object(_) | SchemaKind::OneOf(_)) =>
        {
            let name = if name.is_empty() {
                "{index}".to_owned()
            } else {
                format!("{name}.{{index}}")
            };
            flatten_parameter_schema(output, location, &name, items, false, true, None);
        }
        _ if !name.is_empty() => output.push(ExpandedParameter {
            name: name.to_owned(),
            location,
            required: location == ParameterLocation::Path || required,
            schema: schema.clone(),
            field: field.cloned(),
            indexed,
        }),
        _ => {}
    }
}

fn flatten_one_of(
    output: &mut Vec<ExpandedParameter>,
    location: ParameterLocation,
    prefix: &str,
    variants: &[SchemaMetadata],
    required: bool,
    indexed: bool,
) {
    let mut merged = BTreeMap::<String, (ExpandedParameter, usize)>::new();

    for variant in variants {
        let mut parameters = Vec::new();
        flatten_parameter_schema(
            &mut parameters,
            location,
            prefix,
            variant,
            required,
            indexed,
            None,
        );

        for parameter in parameters {
            match merged.get_mut(&parameter.name) {
                Some((existing, appearances)) => {
                    existing.required &= parameter.required;
                    existing.indexed |= parameter.indexed;
                    existing.schema =
                        merge_parameter_schemas(existing.schema.clone(), parameter.schema.clone());
                    if existing.field != parameter.field {
                        existing.field = None;
                    }
                    *appearances += 1;
                }
                None => {
                    merged.insert(parameter.name.clone(), (parameter, 1));
                }
            }
        }
    }

    for (_, (mut parameter, appearances)) in merged {
        parameter.required &= appearances == variants.len();
        output.push(parameter);
    }
}

fn merge_parameter_schemas(left: SchemaMetadata, right: SchemaMetadata) -> SchemaMetadata {
    if left == right {
        return left;
    }

    let mut alternatives = Vec::new();
    append_schema_alternatives(&mut alternatives, left);
    append_schema_alternatives(&mut alternatives, right);
    alternatives.dedup();

    if alternatives
        .iter()
        .all(|schema| matches!(schema.kind(), SchemaKind::Literal(_)))
    {
        return SchemaMetadata::new(SchemaKind::Enum(
            alternatives
                .into_iter()
                .filter_map(|schema| match schema.kind() {
                    SchemaKind::Literal(value) => Some(value.clone()),
                    _ => None,
                })
                .collect(),
        ));
    }

    SchemaMetadata::new(SchemaKind::OneOf(alternatives))
}

fn append_schema_alternatives(output: &mut Vec<SchemaMetadata>, schema: SchemaMetadata) {
    if let SchemaKind::OneOf(alternatives) = schema.kind() {
        for alternative in alternatives {
            if !output.contains(alternative) {
                output.push(alternative.clone());
            }
        }
    } else if !output.contains(&schema) {
        output.push(schema);
    }
}

impl ParameterLocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
        }
    }
}

fn render_schema(output: &mut String, schema: &SchemaMetadata) {
    if let Some(name) = schema.name() {
        output.push_str("{\"$ref\":");
        let reference = format!("#/components/schemas/{}", json_pointer(name));
        string(output, &reference);
        output.push('}');
        return;
    }

    render_schema_inline(output, schema);
}

fn render_schema_inline(output: &mut String, schema: &SchemaMetadata) {
    output.push('{');

    match schema.kind() {
        SchemaKind::String => output.push_str("\"type\":\"string\""),
        SchemaKind::Integer => output.push_str("\"type\":\"integer\""),
        SchemaKind::Number => output.push_str("\"type\":\"number\""),
        SchemaKind::Boolean => output.push_str("\"type\":\"boolean\""),
        SchemaKind::Bytes => {
            output.push_str("\"type\":\"string\",\"format\":\"binary\"");
        }
        SchemaKind::Enum(values) => {
            output.push_str("\"type\":\"string\",\"enum\":[");
            for (index, value) in values.iter().enumerate() {
                comma(output, index);
                string(output, value);
            }
            output.push(']');
        }
        SchemaKind::Array(items) => {
            output.push_str("\"type\":\"array\",\"items\":");
            render_schema(output, items);
        }
        SchemaKind::Literal(value) => {
            output.push_str("\"type\":\"string\",\"const\":");
            string(output, value);
        }
        SchemaKind::OneOf(schemas) => {
            output.push_str("\"oneOf\":[");
            for (index, schema) in schemas.iter().enumerate() {
                comma(output, index);
                render_schema(output, schema);
            }
            output.push(']');
        }
        SchemaKind::Object(fields) => {
            output.push_str("\"type\":\"object\",\"properties\":{");
            for (index, field) in fields.iter().enumerate() {
                comma(output, index);
                string(output, field.name());
                output.push(':');
                render_field_schema(output, field);
            }
            output.push('}');
            let required = fields
                .iter()
                .filter(|field| field.required())
                .collect::<Vec<_>>();

            if !required.is_empty() {
                output.push_str(",\"required\":[");
                for (index, field) in required.into_iter().enumerate() {
                    comma(output, index);
                    string(output, field.name());
                }
                output.push(']');
            }
        }
    }

    if let Some(format) = schema.format_value() {
        output.push_str(",\"format\":");
        string(output, format);
    }
    if let Some(discriminator) = schema.discriminator_property() {
        output.push_str(",\"discriminator\":{\"propertyName\":");
        string(output, discriminator);
        output.push('}');
    }

    output.push('}');
}

fn json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn render_field_schema(output: &mut String, field: &SchemaField) {
    let mut schema = String::new();
    render_schema(&mut schema, field.schema());
    schema.pop();
    output.push_str(&schema);

    if let Some(minimum) = field.minimum_value() {
        output.push_str(",\"minimum\":");
        output.push_str(minimum);
    }
    if let Some(maximum) = field.maximum_value() {
        output.push_str(",\"maximum\":");
        output.push_str(maximum);
    }
    if let Some(minimum) = field.minimum_length_value() {
        output.push_str(",\"minLength\":");
        output.push_str(&minimum.to_string());
    }
    if let Some(maximum) = field.maximum_length_value() {
        output.push_str(",\"maxLength\":");
        output.push_str(&maximum.to_string());
    }

    output.push('}');
}

fn property(output: &mut String, name: &str, value: &str) {
    string(output, name);
    output.push(':');
    string(output, value);
}

fn string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character < '\u{20}' => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

fn comma(output: &mut String, index: usize) {
    if index > 0 {
        output.push(',');
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApiKeyLocation, ExampleValue, OAuthFlow, OAuthFlows, OpenApi, Operation, ParameterLocation,
        RouteDescription, Scalar, ScalarDeveloperTools, SecurityRequirement, SecurityScheme,
        Server,
    };
    use crate::{Schema, SchemaKind};

    #[derive(crate::Schema)]
    #[allow(dead_code)]
    struct Query {
        #[schema(minimum = 1)]
        page: u32,
        tag: Vec<String>,
    }

    #[derive(crate::Schema)]
    #[allow(dead_code)]
    struct Payload {
        id: u64,
        #[schema(format = "uuid")]
        request_id: String,
        #[schema(nested)]
        selection: Selection,
    }

    #[derive(crate::Schema)]
    #[schema(tag = "type", rename_all = "snake_case")]
    #[allow(dead_code)]
    enum Selection {
        All,
        Range { start: u32, end: u32 },
    }

    #[derive(crate::Schema)]
    #[allow(dead_code)]
    struct Filter {
        name: String,
        limit: Option<u32>,
    }

    #[derive(crate::Schema)]
    #[allow(dead_code)]
    struct ComplexQuery {
        #[schema(nested)]
        selection: Selection,
        #[schema(nested)]
        filters: Vec<Filter>,
    }

    #[test]
    fn renders_valid_route_metadata() {
        let mut operation = Operation::default();
        operation.parameter(ParameterLocation::Query, Query::metadata());
        operation.response(
            200,
            "Success",
            Some("text/plain"),
            Some(crate::SchemaMetadata::new(SchemaKind::String)),
        );
        let document = OpenApi::new("Test", "1.0").build(vec![RouteDescription {
            path: "/items".to_owned(),
            method: "GET".to_owned(),
            path_parameters: Vec::new(),
            operation,
        }]);

        assert!(document.as_str().contains("\"openapi\":\"3.1.0\""));
        assert!(document.as_str().contains("\"page\""));
        assert!(document.as_str().contains("\"minimum\":1"));
    }

    #[test]
    fn escapes_scalar_page_values_for_html_and_script_contexts() {
        let configuration = OpenApi::new("</title><script>", "1.0").scalar_config(
            Scalar::new()
                .theme("moon")
                .show_sidebar(false)
                .developer_tools(ScalarDeveloperTools::Never)
                .default_fonts(false),
        );
        let document = configuration.build(Vec::new());
        let page = configuration.scalar_page(&document);

        assert!(page.contains("<title>&lt;/title&gt;&lt;script&gt;</title>"));
        assert!(page.contains("content:\"{\\\"openapi\\\":\\\"3.1.0\\\""));
        assert!(page.contains("\\u003c/title\\u003e\\u003cscript\\u003e"));
        assert!(page.contains("@scalar/api-reference@1.63.0"));
        assert!(page.contains(",theme:\"moon\""));
        assert!(page.contains(",showSidebar:false"));
        assert!(page.contains(",showDeveloperTools:\"never\""));
        assert!(page.contains(",withDefaultFonts:false"));
        assert!(!page.contains("<title></title><script>"));
    }

    #[test]
    fn renders_components_operation_metadata_security_examples_and_headers() {
        let mut operation = Operation::default();
        operation
            .summary("Create an item")
            .description("Creates one item")
            .operation_id("createItem")
            .tag("items")
            .security(SecurityRequirement::new("bearerAuth").scope("items:write"))
            .request_body("application/json", Some(Payload::metadata()), true)
            .request_example(
                "application/json",
                "sample",
                ExampleValue::object([
                    ("name", ExampleValue::from("request")),
                    ("count", ExampleValue::from(2_u32)),
                    ("active", ExampleValue::from(true)),
                ]),
            )
            .response(
                201,
                "Created",
                Some("application/json"),
                Some(Payload::metadata()),
            )
            .response_header(
                201,
                "Location",
                "Created resource URL",
                crate::SchemaMetadata::new(SchemaKind::String).format("uri"),
            )
            .response_example(
                201,
                "application/json",
                "sample",
                ExampleValue::array([1_u32, 2, 3]),
            );
        let configuration = OpenApi::new("Test", "1.0")
            .server(Server::new("https://api.example.com").description("Production"))
            .security_scheme("bearerAuth", SecurityScheme::bearer().bearer_format("JWT"))
            .security_scheme(
                "apiKey",
                SecurityScheme::api_key("X-API-Key", ApiKeyLocation::Header),
            )
            .security_scheme(
                "oauth",
                SecurityScheme::oauth2(
                    OAuthFlows::new().authorization_code(
                        OAuthFlow::new()
                            .authorization_url("https://example.com/authorize")
                            .token_url("https://example.com/token")
                            .scope("items:write", "Create items"),
                    ),
                ),
            )
            .security_scheme(
                "openid",
                SecurityScheme::open_id_connect(
                    "https://example.com/.well-known/openid-configuration",
                ),
            )
            .security(SecurityRequirement::new("bearerAuth"));
        let document = configuration.build(vec![RouteDescription {
            path: "/items".to_owned(),
            method: "POST".to_owned(),
            path_parameters: Vec::new(),
            operation,
        }]);
        let json = document.as_str();

        assert!(json.contains("\"summary\":\"Create an item\""));
        assert!(json.contains("\"operationId\":\"createItem\""));
        assert!(json.contains("\"components\":{\"schemas\":"));
        assert!(json.contains("\"$ref\":\"#/components/schemas/"));
        assert!(json.contains("\"securitySchemes\""));
        assert!(json.contains("\"authorizationCode\""));
        assert!(json.contains("\"openIdConnectUrl\""));
        assert!(json.contains("\"Location\""));
        assert!(json.contains("\"examples\""));
        assert!(json.contains("\"oneOf\""));
        assert!(json.contains("\"discriminator\":{\"propertyName\":\"type\"}"));

        #[cfg(feature = "json")]
        {
            let document = serde_json::from_str::<serde_json::Value>(json).unwrap();
            let request = &document["paths"]["/items"]["post"]["requestBody"]["content"]["application/json"]
                ["examples"]["sample"]["value"];
            let response = &document["paths"]["/items"]["post"]["responses"]["201"]["content"]["application/json"]
                ["examples"]["sample"]["value"];

            assert_eq!(request["name"], "request");
            assert_eq!(request["count"], 2);
            assert_eq!(request["active"], true);
            assert_eq!(response, &serde_json::json!([1, 2, 3]));
        }
    }

    #[test]
    #[cfg(feature = "json")]
    fn expands_tagged_and_repeated_nested_parameters() {
        let mut operation = Operation::default();
        operation
            .parameter(ParameterLocation::Query, ComplexQuery::metadata())
            .response(200, "Success", None, None);
        let document = OpenApi::new("Test", "1.0").build(vec![RouteDescription {
            path: "/search".to_owned(),
            method: "GET".to_owned(),
            path_parameters: Vec::new(),
            operation,
        }]);
        let document = serde_json::from_str::<serde_json::Value>(document.as_str()).unwrap();
        let parameters = document["paths"]["/search"]["get"]["parameters"]
            .as_array()
            .unwrap();
        let parameter = |name: &str| {
            parameters
                .iter()
                .find(|parameter| parameter["name"] == name)
                .unwrap_or_else(|| panic!("missing `{name}` parameter"))
        };

        assert_eq!(
            parameter("selection.type")["schema"]["enum"],
            serde_json::json!(["all", "range"]),
        );
        assert_eq!(parameter("selection.type")["required"], true);
        assert_eq!(parameter("selection.start")["required"], false);
        assert_eq!(parameter("selection.end")["required"], false);
        assert_eq!(parameter("filters.{index}.name")["required"], false);
        assert_eq!(
            parameter("filters.{index}.name")["x-serverkit-indexed"],
            true,
        );
        assert_eq!(parameter("filters.{index}.limit")["required"], false);
    }
}
