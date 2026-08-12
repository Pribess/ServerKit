use std::fmt;

use crate::{IntoResponse, Response};

#[derive(Debug, Clone, Copy)]
pub struct Value<'value> {
    pub name: &'value str,
    pub bytes: &'value [u8],
}

pub trait Values {
    fn len(&self) -> usize;

    fn value(&self, index: usize) -> Option<Value<'_>>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn name_matches(&self, actual: &str, expected: &str) -> bool {
        actual == expected
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownFields {
    Reject,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeOptions {
    unknown_fields: UnknownFields,
}

impl DecodeOptions {
    pub const fn reject_unknown() -> Self {
        Self {
            unknown_fields: UnknownFields::Reject,
        }
    }

    pub const fn allow_unknown() -> Self {
        Self {
            unknown_fields: UnknownFields::Allow,
        }
    }

    pub const fn unknown_fields(self) -> UnknownFields {
        self.unknown_fields
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self::reject_unknown()
    }
}

pub trait Schema: Sized {
    fn decode<V: Values>(values: &V, options: DecodeOptions) -> Result<Self, ValidationErrors>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationRule {
    Missing,
    UnknownField,
    Multiple,
    InvalidEncoding,
    InvalidType,
    Minimum,
    Maximum,
    MinimumLength,
    MaximumLength,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    field: Option<String>,
    rule: ValidationRule,
    message: String,
}

impl ValidationIssue {
    pub fn custom(message: impl Into<String>) -> Self {
        Self {
            field: None,
            rule: ValidationRule::Custom,
            message: message.into(),
        }
    }

    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    pub fn rule(&self) -> ValidationRule {
        self.rule
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn new(
        field: Option<impl Into<String>>,
        rule: ValidationRule,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field: field.map(Into::into),
            rule,
            message: message.into(),
        }
    }

    #[doc(hidden)]
    pub fn attach_field(mut self, field: &str) -> Self {
        if self.field.is_none() {
            self.field = Some(field.to_owned());
        }

        self
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ValidationErrors {
    issues: Vec<ValidationIssue>,
}

impl ValidationErrors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_issue(issue: ValidationIssue) -> Self {
        Self {
            issues: vec![issue],
        }
    }

    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.issues.len()
    }

    pub(crate) fn push(&mut self, issue: ValidationIssue) {
        self.issues.push(issue);
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }

            if let Some(field) = issue.field() {
                write!(formatter, "{field}: ")?;
            }

            formatter.write_str(issue.message())?;
        }

        Ok(())
    }
}

impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        Response::error(400, self.to_string())
    }
}

#[doc(hidden)]
pub trait ValueSchema: Sized {
    fn decode_value(bytes: &[u8]) -> Result<Self, String>;
}

impl ValueSchema for String {
    fn decode_value(bytes: &[u8]) -> Result<Self, String> {
        String::from_utf8(bytes.to_vec()).map_err(|_| "must be valid UTF-8".to_owned())
    }
}

impl ValueSchema for Vec<u8> {
    fn decode_value(bytes: &[u8]) -> Result<Self, String> {
        Ok(bytes.to_vec())
    }
}

macro_rules! value_schema {
    ($($type:ty),+ $(,)?) => {
        $(
            impl ValueSchema for $type {
                fn decode_value(bytes: &[u8]) -> Result<Self, String> {
                    let value = std::str::from_utf8(bytes)
                        .map_err(|_| "must be valid UTF-8".to_owned())?;

                    value
                        .parse::<Self>()
                        .map_err(|_| format!("must be a valid {}", stringify!($type)))
                }
            }
        )+
    };
}

value_schema!(
    bool, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64,
);

impl<T: ValueSchema> Schema for T {
    fn decode<V: Values>(values: &V, options: DecodeOptions) -> Result<Self, ValidationErrors> {
        let mut decoder = Decoder::new(values, options);
        let value = decoder.single::<T>();
        let errors = decoder.finish();

        match value {
            Some(value) if errors.is_empty() => Ok(value),
            _ => Err(errors),
        }
    }
}

#[doc(hidden)]
pub trait Length {
    fn length(&self) -> usize;
}

impl Length for String {
    fn length(&self) -> usize {
        self.chars().count()
    }
}

impl<T> Length for Vec<T> {
    fn length(&self) -> usize {
        self.len()
    }
}

#[doc(hidden)]
pub struct Decoder<'values, V: Values> {
    values: &'values V,
    options: DecodeOptions,
    consumed: Vec<bool>,
    errors: ValidationErrors,
}

impl<'values, V: Values> Decoder<'values, V> {
    pub fn new(values: &'values V, options: DecodeOptions) -> Self {
        Self {
            values,
            options,
            consumed: vec![false; values.len()],
            errors: ValidationErrors::new(),
        }
    }

    pub fn required<T: ValueSchema>(&mut self, name: &str) -> Option<T> {
        let indexes = self.indexes(name);

        match indexes.as_slice() {
            [] => {
                self.issue(Some(name), ValidationRule::Missing, "is required");
                None
            }
            [index] => self.decode_at::<T>(name, *index),
            _ => {
                self.issue(
                    Some(name),
                    ValidationRule::Multiple,
                    "must appear exactly once",
                );
                None
            }
        }
    }

    pub fn optional<T: ValueSchema>(&mut self, name: &str) -> Option<Option<T>> {
        let indexes = self.indexes(name);

        match indexes.as_slice() {
            [] => Some(None),
            [index] => self.decode_at::<T>(name, *index).map(Some),
            _ => {
                self.issue(
                    Some(name),
                    ValidationRule::Multiple,
                    "must appear at most once",
                );
                None
            }
        }
    }

    pub fn repeated<T: ValueSchema>(&mut self, name: &str) -> Option<Vec<T>> {
        let indexes = self.indexes(name);
        let mut decoded = Vec::with_capacity(indexes.len());
        let mut valid = true;

        for index in indexes {
            match self.decode_at::<T>(name, index) {
                Some(value) => decoded.push(value),
                None => valid = false,
            }
        }

        valid.then_some(decoded)
    }

    pub fn defaulted<T: ValueSchema, F: FnOnce() -> T>(
        &mut self,
        name: &str,
        default: F,
    ) -> Option<T> {
        let indexes = self.indexes(name);

        match indexes.as_slice() {
            [] => Some(default()),
            [index] => self.decode_at::<T>(name, *index),
            _ => {
                self.issue(
                    Some(name),
                    ValidationRule::Multiple,
                    "must appear exactly once",
                );
                None
            }
        }
    }

    pub fn minimum<T: PartialOrd>(&mut self, name: &str, value: &T, minimum: T) {
        if value < &minimum {
            self.issue(Some(name), ValidationRule::Minimum, "is below the minimum");
        }
    }

    pub fn maximum<T: PartialOrd>(&mut self, name: &str, value: &T, maximum: T) {
        if value > &maximum {
            self.issue(Some(name), ValidationRule::Maximum, "is above the maximum");
        }
    }

    pub fn minimum_length<T: Length>(&mut self, name: &str, value: &T, minimum: usize) {
        if value.length() < minimum {
            self.issue(
                Some(name),
                ValidationRule::MinimumLength,
                "is shorter than the minimum length",
            );
        }
    }

    pub fn maximum_length<T: Length>(&mut self, name: &str, value: &T, maximum: usize) {
        if value.length() > maximum {
            self.issue(
                Some(name),
                ValidationRule::MaximumLength,
                "is longer than the maximum length",
            );
        }
    }

    pub fn custom(&mut self, name: &str, result: Result<(), ValidationIssue>) {
        if let Err(issue) = result {
            self.errors.push(issue.attach_field(name));
        }
    }

    pub fn finish(mut self) -> ValidationErrors {
        if self.options.unknown_fields == UnknownFields::Reject {
            let mut unknown = Vec::<String>::new();

            for index in 0..self.values.len() {
                if self.consumed[index] {
                    continue;
                }

                let Some(value) = self.values.value(index) else {
                    continue;
                };

                if unknown.iter().any(|name| name == value.name) {
                    continue;
                }

                unknown.push(value.name.to_owned());
                self.issue(
                    Some(value.name),
                    ValidationRule::UnknownField,
                    "is not declared by the schema",
                );
            }
        }

        self.errors
    }

    fn single<T: ValueSchema>(&mut self) -> Option<T> {
        for consumed in &mut self.consumed {
            *consumed = true;
        }

        match self.values.len() {
            0 => {
                self.issue(None::<&str>, ValidationRule::Missing, "a value is required");
                None
            }
            1 => self.decode_at::<T>("value", 0),
            _ => {
                self.issue(
                    None::<&str>,
                    ValidationRule::Multiple,
                    "exactly one value is required",
                );
                None
            }
        }
    }

    fn indexes(&mut self, name: &str) -> Vec<usize> {
        let indexes = (0..self.values.len())
            .filter(|index| {
                self.values
                    .value(*index)
                    .is_some_and(|value| self.values.name_matches(value.name, name))
            })
            .collect::<Vec<_>>();

        for index in &indexes {
            self.consumed[*index] = true;
        }

        indexes
    }

    fn decode_at<T: ValueSchema>(&mut self, name: &str, index: usize) -> Option<T> {
        let value = self.values.value(index)?;

        match T::decode_value(value.bytes) {
            Ok(value) => Some(value),
            Err(message) => {
                self.issue(Some(name), ValidationRule::InvalidType, message);
                None
            }
        }
    }

    fn issue(
        &mut self,
        field: Option<impl Into<String>>,
        rule: ValidationRule,
        message: impl Into<String>,
    ) {
        self.errors.push(ValidationIssue::new(field, rule, message));
    }
}
