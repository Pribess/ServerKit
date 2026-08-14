use std::collections::BTreeMap;

use crate::{IntoResponse, Response, SchemaField, SchemaKind, SchemaMetadata};

#[derive(Debug, Clone)]
pub struct OpenApi {
    title: String,
    version: String,
}

impl OpenApi {
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
        }
    }

    pub(crate) fn scalar_page(&self, document: &OpenApiDocument) -> String {
        render_scalar_page(&self.title, document.as_str())
    }

    pub(crate) fn build(&self, routes: Vec<RouteDescription>) -> OpenApiDocument {
        OpenApiDocument {
            json: render_document(self, routes),
        }
    }
}

fn render_scalar_page(title: &str, document: &str) -> String {
    let mut output = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>",
    );
    html_text(&mut output, title);
    output.push_str(
        "</title></head><body><div id=\"app\"></div><script src=\"https://cdn.jsdelivr.net/npm/@scalar/api-reference\"></script><script>Scalar.createApiReference('#app',{content:",
    );
    script_string(&mut output, document);
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

#[doc(hidden)]
#[derive(Debug, Default, Clone)]
pub struct Operation {
    parameter_groups: Vec<ParameterGroup>,
    request_body: Option<RequestBody>,
    responses: Vec<ApiResponse>,
}

impl Operation {
    pub fn parameter(&mut self, location: ParameterLocation, schema: SchemaMetadata) {
        self.parameter_groups
            .push(ParameterGroup { location, schema });
    }

    pub fn request_body(
        &mut self,
        content_type: &'static str,
        schema: Option<SchemaMetadata>,
        required: bool,
    ) {
        let request_body = self.request_body.get_or_insert_with(|| RequestBody {
            required,
            content: Vec::new(),
        });
        request_body.required |= required;
        request_body.content.push(MediaType {
            content_type,
            schema,
        });
    }

    pub fn response(
        &mut self,
        status: u16,
        description: &'static str,
        content_type: Option<&'static str>,
        schema: Option<SchemaMetadata>,
    ) {
        if self
            .responses
            .iter()
            .any(|response| response.status == status && response.content_type == content_type)
        {
            return;
        }

        self.responses.push(ApiResponse {
            status,
            description,
            content_type,
            schema,
        });
    }

    pub(crate) fn ensure_response(&mut self) {
        if self.responses.is_empty() {
            self.response(200, "Success", None, None);
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
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
    content_type: &'static str,
    schema: Option<SchemaMetadata>,
}

#[derive(Debug, Clone)]
struct ApiResponse {
    status: u16,
    description: &'static str,
    content_type: Option<&'static str>,
    schema: Option<SchemaMetadata>,
}

#[derive(Debug, Clone)]
pub(crate) struct RouteDescription {
    pub path: String,
    pub method: String,
    pub path_parameters: Vec<String>,
    pub operation: Operation,
}

fn render_document(configuration: &OpenApi, routes: Vec<RouteDescription>) -> String {
    let mut paths = BTreeMap::<String, Vec<RouteDescription>>::new();

    for route in routes {
        paths.entry(route.path.clone()).or_default().push(route);
    }

    let mut output = String::from("{\"openapi\":\"3.1.0\",\"info\":{");
    property(&mut output, "title", &configuration.title);
    output.push(',');
    property(&mut output, "version", &configuration.version);
    output.push_str("},\"paths\":{");

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

    output.push_str("}}");
    output
}

fn render_operation(output: &mut String, route: RouteDescription) {
    output.push_str("\"parameters\":[");
    let parameters = expand_parameters(&route.operation, &route.path_parameters);

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
        output.push('}');
    }

    output.push(']');

    if let Some(request_body) = &route.operation.request_body {
        output.push_str(",\"requestBody\":{\"required\":");
        output.push_str(if request_body.required {
            "true"
        } else {
            "false"
        });
        output.push_str(",\"content\":{");

        for (index, media_type) in request_body.content.iter().enumerate() {
            comma(output, index);
            string(output, media_type.content_type);
            output.push_str(":{");

            if let Some(schema) = &media_type.schema {
                output.push_str("\"schema\":");
                render_schema(output, schema);
            }

            output.push('}');
        }

        output.push_str("}}");
    }

    output.push_str(",\"responses\":{");
    let mut responses = route.operation.responses;
    responses.sort_by_key(|response| response.status);

    for (index, response) in responses.iter().enumerate() {
        comma(output, index);
        string(output, &response.status.to_string());
        output.push_str(":{");
        property(output, "description", response.description);

        if let Some(content_type) = response.content_type {
            output.push_str(",\"content\":{");
            string(output, content_type);
            output.push_str(":{");

            if let Some(schema) = &response.schema {
                output.push_str("\"schema\":");
                render_schema(output, schema);
            }

            output.push_str("}}");
        }

        output.push('}');
    }

    output.push('}');
}

struct ExpandedParameter {
    name: String,
    location: ParameterLocation,
    required: bool,
    schema: SchemaMetadata,
    field: Option<SchemaField>,
}

fn expand_parameters(operation: &Operation, path_parameters: &[String]) -> Vec<ExpandedParameter> {
    let mut parameters = Vec::new();

    for group in &operation.parameter_groups {
        match group.schema.kind() {
            SchemaKind::Object(fields) => {
                flatten_fields(&mut parameters, group.location, "", fields);
            }
            _ if group.location == ParameterLocation::Path && path_parameters.len() == 1 => {
                parameters.push(ExpandedParameter {
                    name: path_parameters[0].clone(),
                    location: group.location,
                    required: true,
                    schema: group.schema.clone(),
                    field: None,
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
) {
    for field in fields {
        let name = if prefix.is_empty() {
            field.name().to_owned()
        } else {
            format!("{prefix}.{}", field.name())
        };

        if let SchemaKind::Object(nested) = field.schema().kind() {
            flatten_fields(output, location, &name, nested);
        } else {
            output.push(ExpandedParameter {
                name,
                location,
                required: location == ParameterLocation::Path || field.required(),
                schema: field.schema().clone(),
                field: Some(field.clone()),
            });
        }
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

    output.push('}');
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
    use super::{OpenApi, Operation, ParameterLocation, RouteDescription};
    use crate::{Schema, SchemaKind};

    #[derive(crate::Schema)]
    #[allow(dead_code)]
    struct Query {
        #[schema(minimum = 1)]
        page: u32,
        tag: Vec<String>,
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
        let configuration = OpenApi::new("</title><script>", "1.0");
        let document = configuration.build(Vec::new());
        let page = configuration.scalar_page(&document);

        assert!(page.contains("<title>&lt;/title&gt;&lt;script&gt;</title>"));
        assert!(page.contains("content:\"{\\\"openapi\\\":\\\"3.1.0\\\""));
        assert!(page.contains("\\u003c/title\\u003e\\u003cscript\\u003e"));
        assert!(!page.contains("<title></title><script>"));
    }
}
