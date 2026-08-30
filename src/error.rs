use std::{error::Error as StdError, fmt, fmt::Write};

use crate::{IntoResponse, Response, ValidationErrors};

#[derive(Debug)]
pub struct Error {
    status: u16,
    code: String,
    message: String,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl Error {
    pub fn new(status: u16, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            source: None,
        }
    }

    pub fn bad_request() -> Self {
        Self::new(400, "bad_request", "Bad Request")
    }

    pub fn unauthorized() -> Self {
        Self::new(401, "unauthorized", "Unauthorized")
    }

    pub fn forbidden() -> Self {
        Self::new(403, "forbidden", "Forbidden")
    }

    pub fn not_found() -> Self {
        Self::new(404, "not_found", "Not Found")
    }

    pub fn conflict() -> Self {
        Self::new(409, "conflict", "Conflict")
    }

    pub fn unprocessable_content() -> Self {
        Self::new(422, "unprocessable_content", "Unprocessable Content")
    }

    pub fn too_many_requests() -> Self {
        Self::new(429, "too_many_requests", "Too Many Requests")
    }

    pub fn internal<E: StdError + Send + Sync + 'static>(source: E) -> Self {
        let message = source.to_string();

        Self {
            status: 500,
            code: "internal_error".to_owned(),
            message,
            source: Some(Box::new(source)),
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
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

    pub fn is_internal(&self) -> bool {
        self.source.is_some()
    }

    pub fn source(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        self.source.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl<E: StdError + Send + Sync + 'static> From<E> for Error {
    fn from(error: E) -> Self {
        Self::internal(error)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        Response::pending_error(self)
    }
}

pub(crate) struct PendingError {
    error: Error,
    validation: Option<ValidationErrors>,
}

impl PendingError {
    pub(crate) fn new(error: Error) -> Self {
        Self {
            error,
            validation: None,
        }
    }

    pub(crate) fn validation(error: Error, validation: ValidationErrors) -> Self {
        Self {
            error,
            validation: Some(validation),
        }
    }

    pub(crate) fn into_parts(self) -> (Error, Option<ValidationErrors>) {
        (self.error, self.validation)
    }
}

pub trait ErrorFormat: Send + Sync + 'static {
    fn format(&self, error: &Error) -> Response;

    #[doc(hidden)]
    fn format_validation(&self, error: &Error, _validation: &ValidationErrors) -> Response {
        self.format(error)
    }
}

impl<F: Fn(&Error) -> Response + Send + Sync + 'static> ErrorFormat for F {
    fn format(&self, error: &Error) -> Response {
        self(error)
    }
}

pub(crate) struct JsonErrorFormat;

impl ErrorFormat for JsonErrorFormat {
    fn format(&self, error: &Error) -> Response {
        json_response(error, None)
    }

    fn format_validation(&self, error: &Error, validation: &ValidationErrors) -> Response {
        json_response(error, Some(validation))
    }
}

fn json_response(error: &Error, validation: Option<&ValidationErrors>) -> Response {
    let mut response = Response::bytes(error.status(), encode_error_json(error, validation));
    response.set_header("Content-Type", "application/json");
    response
}

fn encode_error_json(error: &Error, validation: Option<&ValidationErrors>) -> Vec<u8> {
    let mut output = String::from("{\"error\":{\"code\":");
    write_json_string(&mut output, error.code());
    output.push_str(",\"message\":");
    write_json_string(&mut output, error.message());
    output.push_str(",\"fields\":[");

    if let Some(errors) = validation {
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
    use std::{error::Error as StdError, fmt};

    use super::JsonErrorFormat;
    use crate::{Error, ErrorFormat, ValidationErrors, ValidationIssue};

    #[derive(Debug)]
    struct DatabaseError;

    impl fmt::Display for DatabaseError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("database connection lost")
        }
    }

    impl StdError for DatabaseError {}

    fn database_operation() -> Result<(), DatabaseError> {
        Err(DatabaseError)
    }

    fn propagate_database_error() -> Result<(), Error> {
        database_operation()?;
        Ok(())
    }

    #[test]
    fn renders_the_default_json_envelope_and_escapes_strings() {
        let error = Error::new(400, "request.invalid", "bad \"value\"\n");
        let validation = ValidationErrors::from_issue(ValidationIssue::coded(
            "field.invalid",
            "must not contain \\ escapes",
        ));
        let response = JsonErrorFormat.format_validation(&error, &validation);

        assert_eq!(response.status(), 400);
        assert_eq!(response.content_type(), Some("application/json"));
        assert_eq!(
            response.body(),
            br#"{"error":{"code":"request.invalid","message":"bad \"value\"\n","fields":[{"field":null,"code":"field.invalid","message":"must not contain \\ escapes"}]}}"#,
        );
    }

    #[test]
    fn converts_standard_errors_into_visible_internal_errors() {
        let error = propagate_database_error().unwrap_err();
        let response = JsonErrorFormat.format(&error);

        assert_eq!(error.status(), 500);
        assert_eq!(error.code(), "internal_error");
        assert_eq!(error.message(), "database connection lost");
        assert!(error.is_internal());
        assert_eq!(
            error.source().unwrap().to_string(),
            "database connection lost"
        );
        assert_eq!(
            response.body(),
            br#"{"error":{"code":"internal_error","message":"database connection lost","fields":[]}}"#,
        );
    }

    #[test]
    fn provides_predefined_errors_without_requiring_codes() {
        let cases = [
            (Error::bad_request(), 400, "bad_request", "Bad Request"),
            (Error::unauthorized(), 401, "unauthorized", "Unauthorized"),
            (Error::forbidden(), 403, "forbidden", "Forbidden"),
            (Error::not_found(), 404, "not_found", "Not Found"),
            (Error::conflict(), 409, "conflict", "Conflict"),
            (
                Error::unprocessable_content(),
                422,
                "unprocessable_content",
                "Unprocessable Content",
            ),
            (
                Error::too_many_requests(),
                429,
                "too_many_requests",
                "Too Many Requests",
            ),
        ];

        for (error, status, code, message) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.code(), code);
            assert_eq!(error.message(), message);
            assert!(!error.is_internal());
        }
    }
}
