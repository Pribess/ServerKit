use std::convert::Infallible;

#[cfg(feature = "json")]
use serde::de::DeserializeOwned;

use crate::{
    Body, DecodeOptions, IntoResponse, Method, Request, RequestStream, Response, Schema,
    ValidationErrors, ValidationIssue, ValidationRule, Value, Values,
};

pub struct Unused;
pub struct Buffered;
pub struct Streaming;

pub trait Mode {
    type Input<'request>;
}

impl Mode for Unused {
    type Input<'request> = ();
}

impl Mode for Buffered {
    type Input<'request> = &'request [u8];
}

impl Mode for Streaming {
    type Input<'request> = Box<dyn RequestStream>;
}

pub struct Input<'request, M: Mode> {
    request: &'request Request,
    body: M::Input<'request>,
}

impl<'request, M: Mode> Input<'request, M> {
    pub(crate) fn new(request: &'request Request, body: M::Input<'request>) -> Self {
        Self { request, body }
    }

    pub fn request(&self) -> &'request Request {
        self.request
    }

    pub fn body(self) -> M::Input<'request> {
        self.body
    }
}

pub trait FromRequest<M: Mode = Unused>: Sized {
    type Error: IntoResponse;

    const BUFFERED: bool = false;

    async fn from_request(input: Input<'_, M>) -> Result<Self, Self::Error>;
}

impl FromRequest<Unused> for Method {
    type Error = Infallible;

    async fn from_request(input: Input<'_, Unused>) -> Result<Self, Self::Error> {
        Ok(input.request().method().clone())
    }
}

impl FromRequest<Streaming> for Body {
    type Error = Infallible;

    async fn from_request(input: Input<'_, Streaming>) -> Result<Self, Self::Error> {
        Ok(Body::new(input.body()))
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

impl<T: Schema> FromRequest<Unused> for Path<T> {
    type Error = PathError;

    async fn from_request(input: Input<'_, Unused>) -> Result<Self, Self::Error> {
        let values = PathValues::new(input.request().path_parameters()).map_err(PathError)?;

        T::decode(&values, DecodeOptions::reject_unknown())
            .map(Self)
            .map_err(PathError)
    }
}

impl<T: Schema> FromRequest<Unused> for Query<T> {
    type Error = QueryError;

    async fn from_request(input: Input<'_, Unused>) -> Result<Self, Self::Error> {
        let query = QueryValues::new(input.request().query()).map_err(QueryError)?;

        T::decode(&query, DecodeOptions::reject_unknown())
            .map(Self)
            .map_err(QueryError)
    }
}

impl<T: Schema> FromRequest<Unused> for Header<T> {
    type Error = HeaderError;

    async fn from_request(input: Input<'_, Unused>) -> Result<Self, Self::Error> {
        T::decode(
            &HeaderValues::new(input.request().headers()),
            DecodeOptions::allow_unknown(),
        )
        .map(Self)
        .map_err(HeaderError)
    }
}

struct PathValues {
    values: Vec<(&'static str, Vec<u8>)>,
}

impl PathValues {
    fn new(values: &[(&'static str, String)]) -> Result<Self, ValidationErrors> {
        let values = values
            .iter()
            .map(|(name, value)| {
                decode_url_component(value, false)
                    .map(|value| (*name, value))
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
    Deserialize(serde_json::Error),
}

#[cfg(feature = "json")]
impl IntoResponse for JsonError {
    fn into_response(self) -> Response {
        match self {
            Self::Deserialize(error) => Response::error(400, error.to_string()),
        }
    }
}

#[cfg(feature = "json")]
impl<T: DeserializeOwned> FromRequest<Buffered> for Json<T> {
    type Error = JsonError;

    const BUFFERED: bool = true;

    async fn from_request(input: Input<'_, Buffered>) -> Result<Self, Self::Error> {
        let value = serde_json::from_slice(input.body()).map_err(JsonError::Deserialize)?;

        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use super::{HeaderValues, QueryValues};
    use crate::{DecodeOptions, Headers, Schema, ValidationIssue, ValidationRule};

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
        headers.append("Authorization", "Bearer token");
        headers.append("X-Request-Id", "request-1");
        headers.append("User-Agent", "test");

        let decoded =
            RequestHeaders::decode(&HeaderValues::new(&headers), DecodeOptions::allow_unknown())
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
}
