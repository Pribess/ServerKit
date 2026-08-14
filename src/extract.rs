use std::{convert::Infallible, marker::PhantomData, sync::Arc};

#[cfg(feature = "json")]
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Body, Cookies, DecodeOptions, IntoResponse, Method, Request, Response, Schema, UnknownFields,
    ValidationErrors, ValidationIssue, ValidationRule, Value, Values,
    openapi::{Operation, ParameterLocation},
    schemaval::{SchemaKind, SchemaMetadata},
};

pub trait FromRequest<Input>: Sized {
    type Error: IntoResponse;

    const BUFFERED: bool = false;

    async fn from_request(input: Input) -> Result<Self, Self::Error>;

    #[doc(hidden)]
    fn openapi(_operation: &mut Operation) {}
}

impl<'request> FromRequest<(&'request Request, &'request [u8])> for Method {
    type Error = Infallible;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        Ok(input.0.method().clone())
    }
}

impl FromRequest<Request> for Body {
    type Error = Infallible;

    async fn from_request(input: Request) -> Result<Self, Self::Error> {
        let limit = input.body_limit();
        Ok(Body::new(input.body, limit))
    }

    fn openapi(operation: &mut Operation) {
        operation.request_body(
            "application/octet-stream",
            Some(SchemaMetadata::new(SchemaKind::Bytes)),
            true,
        );
        operation.response(413, "Request body is too large", None, None);
    }
}

pub struct State<T>(pub Arc<T>);

pub struct Extension<T>(pub T);

pub struct ConnectInfo<T>(pub T);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingState<T>(PhantomData<fn() -> T>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingExtension<T>(PhantomData<fn() -> T>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingConnectInfo<T>(PhantomData<fn() -> T>);

macro_rules! missing_value {
    ($type:ident, $message:literal) => {
        impl<T> IntoResponse for $type<T> {
            fn into_response(self) -> Response {
                Response::error(500, $message)
            }
        }
    };
}

missing_value!(MissingState, "application state is unavailable");
missing_value!(MissingExtension, "request extension is unavailable");
missing_value!(MissingConnectInfo, "connection information is unavailable");

impl<'request, T: Send + Sync + 'static> FromRequest<(&'request Request, &'request [u8])>
    for State<T>
{
    type Error = MissingState<T>;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        input
            .0
            .state::<T>()
            .map(Self)
            .ok_or(MissingState(PhantomData))
    }
}

impl<'request, T: Clone + 'static> FromRequest<(&'request Request, &'request [u8])>
    for Extension<T>
{
    type Error = MissingExtension<T>;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        input
            .0
            .extension::<T>()
            .cloned()
            .map(Self)
            .ok_or(MissingExtension(PhantomData))
    }
}

impl<'request, T: Clone + 'static> FromRequest<(&'request Request, &'request [u8])>
    for ConnectInfo<T>
{
    type Error = MissingConnectInfo<T>;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        input
            .0
            .extension::<T>()
            .cloned()
            .map(Self)
            .ok_or(MissingConnectInfo(PhantomData))
    }
}

pub struct Bytes(pub Vec<u8>);

impl<'request> FromRequest<(&'request Request, &'request [u8])> for Bytes {
    type Error = Infallible;

    const BUFFERED: bool = true;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        Ok(Self(input.1.to_vec()))
    }

    fn openapi(operation: &mut Operation) {
        operation.request_body(
            "application/octet-stream",
            Some(SchemaMetadata::new(SchemaKind::Bytes)),
            true,
        );
        operation.response(413, "Request body is too large", None, None);
    }
}

pub struct Text(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextError;

impl IntoResponse for TextError {
    fn into_response(self) -> Response {
        Response::error(400, "request body must be valid UTF-8")
    }
}

impl<'request> FromRequest<(&'request Request, &'request [u8])> for Text {
    type Error = TextError;

    const BUFFERED: bool = true;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        String::from_utf8(input.1.to_vec())
            .map(Self)
            .map_err(|_| TextError)
    }

    fn openapi(operation: &mut Operation) {
        operation.request_body(
            "text/plain",
            Some(SchemaMetadata::new(SchemaKind::String)),
            true,
        );
        operation.response(400, "Invalid UTF-8 request body", None, None);
        operation.response(413, "Request body is too large", None, None);
    }
}

pub struct Form<T>(pub T);

pub enum FormError {
    ContentType,
    Encoding,
    Validation(ValidationErrors),
}

impl IntoResponse for FormError {
    fn into_response(self) -> Response {
        match self {
            Self::ContentType => Response::error(
                415,
                "expected application/x-www-form-urlencoded request body",
            ),
            Self::Encoding => Response::error(400, "form body must be valid UTF-8"),
            Self::Validation(errors) => errors.into_response(),
        }
    }
}

impl<'request, T: Schema> FromRequest<(&'request Request, &'request [u8])> for Form<T> {
    type Error = FormError;

    const BUFFERED: bool = true;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        if !content_type_is(input.0, "application/x-www-form-urlencoded") {
            return Err(FormError::ContentType);
        }

        let body = std::str::from_utf8(input.1).map_err(|_| FormError::Encoding)?;
        let values = QueryValues::new(Some(body)).map_err(FormError::Validation)?;

        T::decode(&values, schema_options::<T>(UnknownFields::Reject))
            .map(Self)
            .map_err(FormError::Validation)
    }

    fn openapi(operation: &mut Operation) {
        operation.request_body(
            "application/x-www-form-urlencoded",
            Some(T::metadata()),
            true,
        );
        operation.response(400, "Invalid form request body", None, None);
        operation.response(413, "Request body is too large", None, None);
        operation.response(415, "Unsupported media type", None, None);
    }
}

impl<'request> FromRequest<(&'request Request, &'request [u8])> for Cookies {
    type Error = Infallible;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        Ok(Cookies::from_headers(input.0.headers()))
    }
}

pub struct Path<T>(pub T);

pub struct Query<T>(pub T);

pub struct Header<T>(pub T);

pub struct PathError(pub ValidationErrors);

pub struct QueryError(pub ValidationErrors);

pub struct HeaderError(pub ValidationErrors);

macro_rules! extractor_error {
    ($type:ty) => {
        impl IntoResponse for $type {
            fn into_response(self) -> Response {
                self.0.into_response()
            }
        }
    };
}

extractor_error!(PathError);
extractor_error!(QueryError);
extractor_error!(HeaderError);

impl<'request, T: Schema> FromRequest<(&'request Request, &'request [u8])> for Path<T> {
    type Error = PathError;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        let values = PathValues::new(input.0.params()).map_err(PathError)?;

        T::decode(&values, schema_options::<T>(UnknownFields::Reject))
            .map(Self)
            .map_err(PathError)
    }

    fn openapi(operation: &mut Operation) {
        operation.parameter(ParameterLocation::Path, T::metadata());
        operation.response(400, "Invalid path parameters", None, None);
    }
}

impl<'request, T: Schema> FromRequest<(&'request Request, &'request [u8])> for Query<T> {
    type Error = QueryError;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        let query = QueryValues::new(input.0.query()).map_err(QueryError)?;

        T::decode(&query, schema_options::<T>(UnknownFields::Ignore))
            .map(Self)
            .map_err(QueryError)
    }

    fn openapi(operation: &mut Operation) {
        operation.parameter(ParameterLocation::Query, T::metadata());
        operation.response(400, "Invalid query parameters", None, None);
    }
}

impl<'request, T: Schema> FromRequest<(&'request Request, &'request [u8])> for Header<T> {
    type Error = HeaderError;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        T::decode(
            &HeaderValues::new(input.0.headers()),
            schema_options::<T>(UnknownFields::Ignore),
        )
        .map(Self)
        .map_err(HeaderError)
    }

    fn openapi(operation: &mut Operation) {
        operation.parameter(ParameterLocation::Header, T::metadata());
        operation.response(400, "Invalid request headers", None, None);
    }
}

fn schema_options<T: Schema>(default: UnknownFields) -> DecodeOptions {
    DecodeOptions::new(T::UNKNOWN_FIELDS.unwrap_or(default))
}

pub(crate) fn content_type(request: &Request) -> Option<&str> {
    request
        .headers()
        .get("content-type")
        .and_then(|value| std::str::from_utf8(value).ok())
}

fn content_type_is(request: &Request, expected: &str) -> bool {
    content_type(request)
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

struct PathValues {
    values: Vec<(String, Vec<u8>)>,
}

impl PathValues {
    fn new(values: &[(String, String)]) -> Result<Self, ValidationErrors> {
        let values = values
            .iter()
            .map(|(name, value)| {
                decode_url_component(value, false)
                    .map(|value| (name.clone(), value))
                    .map_err(|()| invalid_encoding(Some(name)))
            })
            .collect::<Result<_, _>>()?;

        Ok(Self { values })
    }
}

impl Values for PathValues {
    fn len(&self) -> usize {
        self.values.len()
    }

    fn value(&self, index: usize) -> Option<Value<'_>> {
        self.values
            .get(index)
            .map(|(name, value)| Value { name, bytes: value })
    }
}

struct QueryValues {
    values: Vec<(String, Vec<u8>)>,
}

impl QueryValues {
    fn new(query: Option<&str>) -> Result<Self, ValidationErrors> {
        let values = query
            .into_iter()
            .flat_map(|query| query.split('&'))
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                let name = decode_url_component(name, true).map_err(|()| invalid_encoding(None))?;
                let name = String::from_utf8(name).map_err(|_| invalid_encoding(None))?;
                let value = decode_url_component(value, true)
                    .map_err(|()| invalid_encoding(Some(&name)))?;

                Ok((name, value))
            })
            .collect::<Result<_, ValidationErrors>>()?;

        Ok(Self { values })
    }
}

impl Values for QueryValues {
    fn len(&self) -> usize {
        self.values.len()
    }

    fn value(&self, index: usize) -> Option<Value<'_>> {
        self.values
            .get(index)
            .map(|(name, value)| Value { name, bytes: value })
    }
}

struct HeaderValues<'request> {
    headers: &'request crate::Headers,
}

impl<'request> HeaderValues<'request> {
    fn new(headers: &'request crate::Headers) -> Self {
        Self { headers }
    }
}

impl Values for HeaderValues<'_> {
    fn len(&self) -> usize {
        self.headers.len()
    }

    fn value(&self, index: usize) -> Option<Value<'_>> {
        self.headers
            .iter()
            .nth(index)
            .map(|(name, bytes)| Value { name, bytes })
    }

    fn name_matches(&self, actual: &str, expected: &str) -> bool {
        actual.eq_ignore_ascii_case(expected)
    }

    fn names_are_case_insensitive(&self) -> bool {
        true
    }

    fn strip_name_prefix<'name>(&self, actual: &'name str, prefix: &str) -> Option<&'name str> {
        actual
            .get(..prefix.len())
            .filter(|actual| actual.eq_ignore_ascii_case(prefix))
            .and_then(|_| actual.get(prefix.len()..))
    }
}

fn decode_url_component(value: &str, plus_as_space: bool) -> Result<Vec<u8>, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' if plus_as_space => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                let (Some(high), Some(low)) = (
                    bytes.get(index + 1).copied().and_then(hex),
                    bytes.get(index + 2).copied().and_then(hex),
                ) else {
                    return Err(());
                };

                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    Ok(decoded)
}

fn invalid_encoding(field: Option<&str>) -> ValidationErrors {
    ValidationErrors::from_issue(ValidationIssue::new(
        field,
        ValidationRule::InvalidEncoding,
        "contains invalid percent encoding",
    ))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(feature = "json")]
pub struct Json<T>(pub T);

#[cfg(feature = "json")]
pub enum JsonError {
    ContentType,
    Deserialize(serde_json::Error),
}

#[cfg(feature = "json")]
impl IntoResponse for JsonError {
    fn into_response(self) -> Response {
        match self {
            Self::ContentType => Response::error(415, "expected application/json request body"),
            Self::Deserialize(error) => Response::error(400, error.to_string()),
        }
    }
}

#[cfg(feature = "json")]
impl<'request, T: DeserializeOwned + Schema> FromRequest<(&'request Request, &'request [u8])>
    for Json<T>
{
    type Error = JsonError;

    const BUFFERED: bool = true;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        if !content_type_is(input.0, "application/json")
            && !content_type(input.0).is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|value| value.trim().ends_with("+json"))
            })
        {
            return Err(JsonError::ContentType);
        }

        let value = serde_json::from_slice(input.1).map_err(JsonError::Deserialize)?;

        Ok(Self(value))
    }

    fn openapi(operation: &mut Operation) {
        operation.request_body("application/json", Some(T::metadata()), true);
        operation.response(400, "Invalid JSON request body", None, None);
        operation.response(413, "Request body is too large", None, None);
        operation.response(415, "Unsupported media type", None, None);
    }
}

#[cfg(feature = "json")]
impl<T: Serialize + Schema> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(body) => {
                let mut response = Response::bytes(200, body);
                response.set_header("Content-Type", "application/json");
                response
            }
            Err(error) => Response::error(500, error.to_string()),
        }
    }

    fn openapi(operation: &mut Operation) {
        operation.response(
            200,
            "Success",
            Some("application/json"),
            Some(T::metadata()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{HeaderValues, PathValues, QueryValues};
    use crate::{
        DecodeOptions, ExtraFields, Headers, Schema, UnknownFields, ValidationIssue, ValidationRule,
    };

    #[derive(Debug, PartialEq, crate::Schema)]
    struct SearchQuery {
        #[schema(rename = "q", min_length = 2)]
        term: String,
        #[schema(default = 1, minimum = 1, maximum = 100)]
        page: u32,
        tag: Vec<String>,
        exact: Option<bool>,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    #[schema(rename_all = "kebab-case")]
    struct RequestHeaders {
        authorization: String,
        x_request_id: Option<String>,
    }

    fn validate_slug(value: &str) -> Result<(), ValidationIssue> {
        value
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '-')
            .then_some(())
            .ok_or_else(|| ValidationIssue::custom("must be a lowercase slug"))
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct Slug {
        #[schema(validate = validate_slug)]
        slug: String,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    #[schema(unknown_fields = "reject")]
    struct StrictInput {
        value: String,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    #[schema(unknown_fields = "ignore")]
    struct FlexibleInput {
        value: String,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct CapturedInput {
        value: String,
        #[schema(rest)]
        extra: ExtraFields,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct RestFirstInput {
        #[schema(rest)]
        extra: ExtraFields,
        value: String,
    }

    #[test]
    fn query_schema_decodes_named_fields_and_defaults() {
        let values = QueryValues::new(Some("q=rust&page=2&tag=web&tag=server&exact=true")).unwrap();
        let decoded = SearchQuery::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(
            decoded,
            SearchQuery {
                term: "rust".to_owned(),
                page: 2,
                tag: vec!["web".to_owned(), "server".to_owned()],
                exact: Some(true),
            }
        );

        let values = QueryValues::new(Some("q=rust")).unwrap();
        let decoded = SearchQuery::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.page, 1);
        assert!(decoded.tag.is_empty());
        assert_eq!(decoded.exact, None);
    }

    #[test]
    fn query_schema_rejects_unknown_fields() {
        let values = QueryValues::new(Some("q=rust&debug=true")).unwrap();
        let errors = SearchQuery::decode(&values, DecodeOptions::reject_unknown()).unwrap_err();

        assert!(errors.issues().iter().any(|issue| {
            issue.field() == Some("debug") && issue.rule() == ValidationRule::UnknownField
        }));
    }

    #[test]
    fn header_schema_allows_unknown_fields_and_matches_case_insensitively() {
        let mut headers = Headers::new();
        headers.append("Authorization", "Bearer token").unwrap();
        headers.append("X-Request-Id", "request-1").unwrap();
        headers.append("User-Agent", "test").unwrap();

        let decoded = RequestHeaders::decode(
            &HeaderValues::new(&headers),
            DecodeOptions::ignore_unknown(),
        )
        .unwrap();

        assert_eq!(decoded.authorization, "Bearer token");
        assert_eq!(decoded.x_request_id.as_deref(), Some("request-1"));
    }

    #[test]
    fn custom_validation_is_reported_with_the_field_name() {
        let values = QueryValues::new(Some("slug=Not-Valid")).unwrap();
        let errors = Slug::decode(&values, DecodeOptions::reject_unknown()).unwrap_err();

        assert_eq!(errors.issues()[0].field(), Some("slug"));
        assert_eq!(errors.issues()[0].rule(), ValidationRule::Custom);
    }

    #[test]
    fn schema_exposes_an_optional_unknown_field_override() {
        assert_eq!(<SearchQuery as Schema>::UNKNOWN_FIELDS, None,);
        assert_eq!(
            <StrictInput as Schema>::UNKNOWN_FIELDS,
            Some(UnknownFields::Reject),
        );
        assert_eq!(
            <FlexibleInput as Schema>::UNKNOWN_FIELDS,
            Some(UnknownFields::Ignore),
        );
    }

    #[test]
    fn rest_captures_decoded_query_values_and_duplicates() {
        let values = QueryValues::new(Some(
            "value=known&debug=true&tag=web&tag=server&message=hello+world",
        ))
        .unwrap();
        let decoded = CapturedInput::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.value, "known");
        assert_eq!(decoded.extra.get("debug"), Some(b"true".as_slice()));
        assert_eq!(
            decoded.extra.get_all("tag").collect::<Vec<_>>(),
            vec![b"web".as_slice(), b"server".as_slice()],
        );
        assert_eq!(
            decoded.extra.get("message"),
            Some(b"hello world".as_slice()),
        );
        assert_eq!(decoded.extra.len(), 4);
    }

    #[test]
    fn rest_uses_case_insensitive_header_names() {
        let mut headers = Headers::new();
        headers.append("Value", "known").unwrap();
        headers.append("X-Trace-Id", "trace-1").unwrap();
        let decoded = CapturedInput::decode(
            &HeaderValues::new(&headers),
            DecodeOptions::reject_unknown(),
        )
        .unwrap();

        assert_eq!(decoded.extra.get("x-trace-id"), Some(b"trace-1".as_slice()));
        assert_eq!(decoded.extra.get("X-TRACE-ID"), Some(b"trace-1".as_slice()));
    }

    #[test]
    fn rejected_header_names_are_deduplicated_case_insensitively() {
        let mut headers = Headers::new();
        headers.append("Value", "known").unwrap();
        headers.append("X-Extra", "first").unwrap();
        headers.append("x-extra", "second").unwrap();
        let errors = StrictInput::decode(
            &HeaderValues::new(&headers),
            DecodeOptions::reject_unknown(),
        )
        .unwrap_err();

        assert_eq!(
            errors
                .issues()
                .iter()
                .filter(|issue| issue.rule() == ValidationRule::UnknownField)
                .count(),
            1,
        );
    }

    #[test]
    fn rest_captures_percent_decoded_path_values() {
        let values = PathValues::new(&[
            ("value".to_owned(), "known".to_owned()),
            ("slug".to_owned(), "hello%20world".to_owned()),
        ])
        .unwrap();
        let decoded = CapturedInput::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.extra.get("slug"), Some(b"hello world".as_slice()));
    }

    #[test]
    fn rest_only_captures_unknown_values_regardless_of_field_order() {
        let values = QueryValues::new(Some("extra=captured&value=known")).unwrap();
        let decoded = RestFirstInput::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.value, "known");
        assert_eq!(decoded.extra.get("extra"), Some(b"captured".as_slice()));
        assert_eq!(decoded.extra.get("value"), None);
    }
}
