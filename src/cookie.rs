use std::fmt;

use crate::Headers;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    name: String,
    value: String,
    path: Option<String>,
    domain: Option<String>,
    max_age: Option<i64>,
    same_site: Option<SameSite>,
    http_only: bool,
    secure: bool,
}

impl Cookie {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            path: None,
            domain: None,
            max_age: None,
            same_site: None,
            http_only: false,
            secure: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = Some(same_site);
        self
    }

    pub fn http_only(mut self, enabled: bool) -> Self {
        self.http_only = enabled;
        self
    }

    pub fn secure(mut self, enabled: bool) -> Self {
        self.secure = enabled;
        self
    }

    pub(crate) fn header_value(&self) -> String {
        let mut value = format!("{}={}", self.name, self.value);

        if let Some(path) = &self.path {
            value.push_str("; Path=");
            value.push_str(path);
        }

        if let Some(domain) = &self.domain {
            value.push_str("; Domain=");
            value.push_str(domain);
        }

        if let Some(max_age) = self.max_age {
            value.push_str("; Max-Age=");
            value.push_str(&max_age.to_string());
        }

        if let Some(same_site) = self.same_site {
            value.push_str("; SameSite=");
            value.push_str(same_site.as_str());
        }

        if self.http_only {
            value.push_str("; HttpOnly");
        }

        if self.secure {
            value.push_str("; Secure");
        }

        value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cookies {
    entries: Vec<(String, String)>,
}

impl Cookies {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(actual, _)| actual == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn get_all<'cookies>(
        &'cookies self,
        name: &'cookies str,
    ) -> impl Iterator<Item = &'cookies str> + 'cookies {
        self.entries
            .iter()
            .filter(move |(actual, _)| actual == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn from_headers(headers: &Headers) -> Self {
        let entries = headers
            .get_all("cookie")
            .filter_map(|value| std::str::from_utf8(value).ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| {
                let (name, value) = pair.trim().split_once('=')?;
                (!name.is_empty()).then(|| (name.to_owned(), unquote(value)))
            })
            .collect();

        Self { entries }
    }
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

impl fmt::Display for Cookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.header_value())
    }
}

#[cfg(test)]
mod tests {
    use super::{Cookie, Cookies, SameSite};
    use crate::Headers;

    #[test]
    fn parses_repeated_cookie_headers() {
        let mut headers = Headers::new();
        headers.append("Cookie", "session=abc; theme=dark").unwrap();
        headers.append("cookie", "tag=one").unwrap();
        let cookies = Cookies::from_headers(&headers);

        assert_eq!(cookies.get("session"), Some("abc"));
        assert_eq!(cookies.get("theme"), Some("dark"));
        assert_eq!(cookies.get("tag"), Some("one"));
    }

    #[test]
    fn formats_set_cookie_attributes() {
        let cookie = Cookie::new("session", "abc")
            .path("/")
            .max_age(60)
            .same_site(SameSite::Lax)
            .http_only(true)
            .secure(true);

        assert_eq!(
            cookie.to_string(),
            "session=abc; Path=/; Max-Age=60; SameSite=Lax; HttpOnly; Secure",
        );
    }
}
