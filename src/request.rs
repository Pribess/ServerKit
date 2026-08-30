use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    str::FromStr,
    sync::Arc,
};

use crate::{Error, IntoResponse, RequestStream, Response};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method(MethodRepr);

#[derive(Debug, Clone, PartialEq, Eq)]
enum MethodRepr {
    Connect,
    Delete,
    Get,
    Head,
    Options,
    Patch,
    Post,
    Put,
    Trace,
    Other(Box<str>),
}

impl Method {
    pub const CONNECT: Self = Self(MethodRepr::Connect);
    pub const DELETE: Self = Self(MethodRepr::Delete);
    pub const GET: Self = Self(MethodRepr::Get);
    pub const HEAD: Self = Self(MethodRepr::Head);
    pub const OPTIONS: Self = Self(MethodRepr::Options);
    pub const PATCH: Self = Self(MethodRepr::Patch);
    pub const POST: Self = Self(MethodRepr::Post);
    pub const PUT: Self = Self(MethodRepr::Put);
    pub const TRACE: Self = Self(MethodRepr::Trace);

    pub fn from_bytes(value: &[u8]) -> Result<Self, InvalidMethod> {
        if !valid_method(value) {
            return Err(InvalidMethod);
        }

        let value = std::str::from_utf8(value).map_err(|_| InvalidMethod)?;
        Ok(Self::standard(value).unwrap_or_else(|| Self(MethodRepr::Other(value.into()))))
    }

    pub fn as_str(&self) -> &str {
        match &self.0 {
            MethodRepr::Connect => "CONNECT",
            MethodRepr::Delete => "DELETE",
            MethodRepr::Get => "GET",
            MethodRepr::Head => "HEAD",
            MethodRepr::Options => "OPTIONS",
            MethodRepr::Patch => "PATCH",
            MethodRepr::Post => "POST",
            MethodRepr::Put => "PUT",
            MethodRepr::Trace => "TRACE",
            MethodRepr::Other(method) => method,
        }
    }

    pub(crate) fn is_openapi_operation(&self) -> bool {
        !matches!(self.0, MethodRepr::Connect | MethodRepr::Other(_))
    }

    fn standard(value: &str) -> Option<Self> {
        match value {
            "CONNECT" => Some(Self::CONNECT),
            "DELETE" => Some(Self::DELETE),
            "GET" => Some(Self::GET),
            "HEAD" => Some(Self::HEAD),
            "OPTIONS" => Some(Self::OPTIONS),
            "PATCH" => Some(Self::PATCH),
            "POST" => Some(Self::POST),
            "PUT" => Some(Self::PUT),
            "TRACE" => Some(Self::TRACE),
            _ => None,
        }
    }
}

impl FromStr for Method {
    type Err = InvalidMethod;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(value.as_bytes())
    }
}

impl TryFrom<&str> for Method {
    type Error = InvalidMethod;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(value.as_bytes())
    }
}

impl TryFrom<String> for Method {
    type Error = InvalidMethod;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !valid_method(value.as_bytes()) {
            return Err(InvalidMethod);
        }

        Ok(Self::standard(&value)
            .unwrap_or_else(|| Self(MethodRepr::Other(value.into_boxed_str()))))
    }
}

impl AsRef<str> for Method {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMethod;

impl InvalidMethod {
    pub fn message(&self) -> &'static str {
        "invalid HTTP method"
    }
}

impl fmt::Display for InvalidMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for InvalidMethod {}

fn valid_method(value: &[u8]) -> bool {
    !value.is_empty()
        && value.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

#[cfg(test)]
mod method_tests {
    use super::Method;

    #[test]
    fn exposes_standard_methods_as_constants() {
        let methods = [
            (Method::CONNECT, "CONNECT"),
            (Method::DELETE, "DELETE"),
            (Method::GET, "GET"),
            (Method::HEAD, "HEAD"),
            (Method::OPTIONS, "OPTIONS"),
            (Method::PATCH, "PATCH"),
            (Method::POST, "POST"),
            (Method::PUT, "PUT"),
            (Method::TRACE, "TRACE"),
        ];

        for (method, name) in methods {
            assert_eq!(method.as_str(), name);
            assert_eq!(Method::from_bytes(name.as_bytes()).unwrap(), method);
        }
    }

    #[test]
    fn preserves_other_methods() {
        let method = Method::from_bytes(b"PROPFIND").unwrap();

        assert_eq!(method.as_str(), "PROPFIND");
    }

    #[test]
    fn rejects_invalid_method_tokens() {
        for method in [b"".as_slice(), b"NOT VALID", b"GET/POST", b"m\xc3\xa9thod"] {
            assert!(Method::from_bytes(method).is_err());
        }
    }
}

#[derive(Debug, Default)]
pub struct Headers {
    entries: Vec<(String, Vec<u8>)>,
}

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_slice())
    }

    pub fn get_all<'headers>(
        &'headers self,
        name: &'headers str,
    ) -> impl Iterator<Item = &'headers [u8]> + 'headers {
        self.entries
            .iter()
            .filter(move |(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_slice())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|(header, _)| header.eq_ignore_ascii_case(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice()))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn set(
        &mut self,
        name: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), InvalidHeader> {
        let name = name.into();
        let value = value.into();

        validate_header(&name, &value)?;
        self.remove(&name);
        self.entries.push((name, value));

        Ok(())
    }

    pub fn append(
        &mut self,
        name: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), InvalidHeader> {
        let name = name.into();
        let value = value.into();

        validate_header(&name, &value)?;
        self.entries.push((name, value));

        Ok(())
    }

    pub fn remove(&mut self, name: &str) {
        self.entries
            .retain(|(header, _)| !header.eq_ignore_ascii_case(name));
    }

    pub(crate) fn append_unchecked(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.entries.push((name.into(), value.into()));
    }

    pub(crate) fn set_unchecked(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        let name = name.into();
        self.remove(&name);
        self.entries.push((name, value.into()));
    }

    pub(crate) fn merge_from(&mut self, headers: Self) {
        self.entries.retain(|(existing, _)| {
            !headers
                .entries
                .iter()
                .any(|(incoming, _)| existing.eq_ignore_ascii_case(incoming))
        });
        self.entries.extend(headers.entries);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHeader {
    message: &'static str,
}

impl InvalidHeader {
    pub fn message(&self) -> &str {
        self.message
    }
}

impl fmt::Display for InvalidHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl IntoResponse for InvalidHeader {
    fn into_response(self) -> Response {
        Error::new(500, "response.header.invalid", self.message).into_response()
    }
}

fn validate_header(name: &str, value: &[u8]) -> Result<(), InvalidHeader> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(InvalidHeader {
            message: "invalid HTTP header name",
        });
    }

    if value
        .iter()
        .any(|byte| matches!(byte, b'\0' | b'\r' | b'\n'))
    {
        return Err(InvalidHeader {
            message: "invalid HTTP header value",
        });
    }

    Ok(())
}

pub struct Request {
    pub method: Method,
    pub path: String,
    pub query: Option<String>,
    pub headers: Headers,
    params: Vec<(String, String)>,
    extensions: HashMap<TypeId, Box<dyn Any>>,
    states: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    body_limit: Option<usize>,
    pub(crate) body: Box<dyn RequestStream>,
}

impl Request {
    pub fn from_parts(
        method: Method,
        path: impl Into<String>,
        query: Option<String>,
        headers: Headers,
        body: Box<dyn RequestStream>,
    ) -> Self {
        Self {
            method,
            path: path.into(),
            query,
            headers,
            params: Vec::new(),
            extensions: HashMap::new(),
            states: HashMap::new(),
            body_limit: None,
            body,
        }
    }

    pub(crate) fn params(&self) -> &[(String, String)] {
        &self.params
    }

    pub(crate) fn set_params(&mut self, params: Vec<(String, String)>) {
        self.params = params;
    }

    pub fn insert_extension<T: 'static>(&mut self, value: T) {
        self.extensions.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn extension<T: 'static>(&self) -> Option<&T> {
        self.extensions
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref())
    }

    pub(crate) fn set_states(&mut self, states: HashMap<TypeId, Arc<dyn Any + Send + Sync>>) {
        self.states = states;
    }

    pub(crate) fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.states
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|state| state.downcast().ok())
    }

    pub(crate) fn set_body_limit(&mut self, limit: Option<usize>) {
        self.body_limit = limit;
    }

    pub(crate) fn body_limit(&self) -> Option<usize> {
        self.body_limit
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Request")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("headers", &self.headers)
            .field("params", &self.params)
            .field("body_limit", &self.body_limit)
            .finish_non_exhaustive()
    }
}
