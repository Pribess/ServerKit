use std::{fmt, fmt::Write};

use crate::{Headers, IntoResponse, Response, ValidationErrors};

#[derive(Debug)]
pub struct HttpError {
    status: u16,
    code: String,
    message: String,
    validation: Option<ValidationErrors>,
    headers: Headers,
}

impl HttpError {
    pub fn new(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            validation: None,
            headers: Headers::new(),
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn validation(mut self, errors: ValidationErrors) -> Self {
        self.validation = Some(errors);
        self
    }

    pub fn validation_errors(&self) -> Option<&ValidationErrors> {
        self.validation.as_ref()
    }

    pub fn headers(&mut self) -> &mut Headers {
        &mut self.headers
    }

    pub(crate) fn take_headers(&mut self) -> Headers {
        std::mem::take(&mut self.headers)
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpError {}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        Response::pending_error(self)
    }
}

pub trait ErrorFormat: Send + Sync + 'static {
    fn format(&self, error: &HttpError) -> Response;
}

impl<F: Fn(&HttpError) -> Response + Send + Sync + 'static> ErrorFormat for F {
    fn format(&self, error: &HttpError) -> Response {
        self(error)
    }
}

pub(crate) struct JsonErrorFormat;

impl ErrorFormat for JsonErrorFormat {
    fn format(&self, error: &HttpError) -> Response {
        let mut response = Response::bytes(error.status(), encode_error_json(error));
        response.set_header("Content-Type", "application/json");
        response
    }
}

fn encode_error_json(error: &HttpError) -> Vec<u8> {
    let mut output = String::from("{\"error\":{\"code\":");
    write_json_string(&mut output, error.code());
    output.push_str(",\"message\":");
    write_json_string(&mut output, error.message());
    output.push_str(",\"fields\":[");

    if let Some(errors) = error.validation_errors() {
        for (index, issue) in errors.issues().iter().enumerate() {
            if index != 0 {
                output.push(',');
            }

            output.push_str("{\"field\":");
            match issue.field() {
                Some(field) => write_json_string(&mut output, field),
                None => output.push_str("null"),
            }
            output.push_str(",\"code\":");
            write_json_string(&mut output, issue.code());
            output.push_str(",\"message\":");
            write_json_string(&mut output, issue.message());
            output.push('}');
        }
    }

    output.push_str("]}}");
    output.into_bytes()
}

fn write_json_string(output: &mut String, value: &str) {
    output.push('"');

    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            character if character < '\u{20}' => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to a String is infallible");
            }
            character => output.push(character),
        }
    }

    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::JsonErrorFormat;
    use crate::{ErrorFormat, HttpError, ValidationErrors, ValidationIssue};

    #[test]
    fn renders_the_default_json_envelope_and_escapes_strings() {
        let error = HttpError::new(400, "request.invalid", "bad \"value\"\n").validation(
            ValidationErrors::from_issue(ValidationIssue::coded(
                "field.invalid",
                "must not contain \\ escapes",
            )),
        );
        let response = JsonErrorFormat.format(&error);

        assert_eq!(response.status(), 400);
        assert_eq!(response.content_type(), Some("application/json"));
        assert_eq!(
            response.body(),
            br#"{"error":{"code":"request.invalid","message":"bad \"value\"\n","fields":[{"field":null,"code":"field.invalid","message":"must not contain \\ escapes"}]}}"#,
        );
    }
}
