use crate::{IntoResponse, Response, openapi::Operation};

pub struct Redirect {
    status: u16,
    location: String,
}

impl Redirect {
    pub fn temporary(location: impl Into<String>) -> Self {
        Self {
            status: 307,
            location: location.into(),
        }
    }

    pub fn permanent(location: impl Into<String>) -> Self {
        Self {
            status: 308,
            location: location.into(),
        }
    }

    pub fn see_other(location: impl Into<String>) -> Self {
        Self {
            status: 303,
            location: location.into(),
        }
    }

    pub fn found(location: impl Into<String>) -> Self {
        Self {
            status: 302,
            location: location.into(),
        }
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> Response {
        let mut response = Response::new(self.status);

        if let Err(error) = response.headers().set("Location", self.location) {
            return error.into_response();
        }

        response
    }

    fn openapi(operation: &mut Operation) {
        operation.response(307, "Redirect", None, None);
    }
}
