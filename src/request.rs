#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method(String);

impl Method {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

    pub(crate) fn append(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.entries.push((name.into(), value.into()));
    }
}

#[derive(Debug)]
pub struct Request {
    method: Method,
    path: String,
    query: Option<String>,
    headers: Headers,
    path_parameters: Vec<(&'static str, String)>,
}

impl Request {
    pub(crate) fn new(
        method: Method,
        path: impl Into<String>,
        query: Option<String>,
        headers: Headers,
    ) -> Self {
        Self {
            method,
            path: path.into(),
            query,
            headers,
            path_parameters: Vec::new(),
        }
    }

    pub fn method(&self) -> &Method {
        &self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    pub(crate) fn path_parameters(&self) -> &[(&'static str, String)] {
        &self.path_parameters
    }

    pub(crate) fn set_path_parameters(&mut self, path_parameters: Vec<(&'static str, String)>) {
        self.path_parameters = path_parameters;
    }
}
