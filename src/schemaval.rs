use std::{collections::BTreeSet, fmt, net};

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

    fn names_are_case_insensitive(&self) -> bool {
        false
    }

    fn strip_name_prefix<'name>(&self, actual: &'name str, prefix: &str) -> Option<&'name str> {
        actual.strip_prefix(prefix)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownFields {
    Reject,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeOptions {
    unknown_fields: UnknownFields,
}

impl DecodeOptions {
    pub const fn new(unknown_fields: UnknownFields) -> Self {
        Self { unknown_fields }
    }

    pub const fn reject_unknown() -> Self {
        Self::new(UnknownFields::Reject)
    }

    pub const fn ignore_unknown() -> Self {
        Self::new(UnknownFields::Ignore)
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
    const UNKNOWN_FIELDS: Option<UnknownFields> = None;

    fn decode<V: Values>(values: &V, options: DecodeOptions) -> Result<Self, ValidationErrors>;

    fn metadata() -> SchemaMetadata;
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaMetadata {
    name: Option<String>,
    kind: SchemaKind,
    format: Option<String>,
    discriminator: Option<String>,
}

impl SchemaMetadata {
    pub fn new(kind: SchemaKind) -> Self {
        Self {
            name: None,
            kind,
            format: None,
            discriminator: None,
        }
    }

    pub fn named(name: impl Into<String>, kind: SchemaKind) -> Self {
        Self {
            name: Some(name.into()),
            kind,
            format: None,
            discriminator: None,
        }
    }

    pub fn array(items: Self) -> Self {
        Self::new(SchemaKind::Array(Box::new(items)))
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    pub fn discriminator(mut self, property: impl Into<String>) -> Self {
        self.discriminator = Some(property.into());
        self
    }

    pub fn kind(&self) -> &SchemaKind {
        &self.kind
    }

    pub fn format_value(&self) -> Option<&str> {
        self.format.as_deref()
    }

    pub fn discriminator_property(&self) -> Option<&str> {
        self.discriminator.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SchemaKind {
    String,
    Integer,
    Number,
    Boolean,
    Bytes,
    Object(Vec<SchemaField>),
    Enum(Vec<String>),
    Array(Box<SchemaMetadata>),
    Literal(String),
    OneOf(Vec<SchemaMetadata>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaField {
    name: String,
    schema: SchemaMetadata,
    required: bool,
    minimum: Option<String>,
    maximum: Option<String>,
    minimum_length: Option<usize>,
    maximum_length: Option<usize>,
}

impl SchemaField {
    pub fn new(name: impl Into<String>, schema: SchemaMetadata, required: bool) -> Self {
        Self {
            name: name.into(),
            schema,
            required,
            minimum: None,
            maximum: None,
            minimum_length: None,
            maximum_length: None,
        }
    }

    pub fn minimum(mut self, minimum: impl ToString) -> Self {
        self.minimum = Some(minimum.to_string());
        self
    }

    pub fn maximum(mut self, maximum: impl ToString) -> Self {
        self.maximum = Some(maximum.to_string());
        self
    }

    pub fn minimum_length(mut self, minimum: usize) -> Self {
        self.minimum_length = Some(minimum);
        self
    }

    pub fn maximum_length(mut self, maximum: usize) -> Self {
        self.maximum_length = Some(maximum);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn schema(&self) -> &SchemaMetadata {
        &self.schema
    }

    pub fn required(&self) -> bool {
        self.required
    }

    pub fn minimum_value(&self) -> Option<&str> {
        self.minimum.as_deref()
    }

    pub fn maximum_value(&self) -> Option<&str> {
        self.maximum.as_deref()
    }

    pub fn minimum_length_value(&self) -> Option<usize> {
        self.minimum_length
    }

    pub fn maximum_length_value(&self) -> Option<usize> {
        self.maximum_length
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExtraFields {
    entries: Vec<(String, Vec<u8>)>,
    case_insensitive: bool,
}

impl ExtraFields {
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(actual, _)| self.name_matches(actual, name))
            .map(|(_, value)| value.as_slice())
    }

    pub fn get_all<'fields>(
        &'fields self,
        name: &'fields str,
    ) -> impl Iterator<Item = &'fields [u8]> + 'fields {
        self.entries
            .iter()
            .filter(move |(actual, _)| self.name_matches(actual, name))
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

    fn name_matches(&self, actual: &str, expected: &str) -> bool {
        if self.case_insensitive {
            actual.eq_ignore_ascii_case(expected)
        } else {
            actual == expected
        }
    }
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

impl ValidationRule {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::UnknownField => "unknown_field",
            Self::Multiple => "multiple",
            Self::InvalidEncoding => "invalid_encoding",
            Self::InvalidType => "invalid_type",
            Self::Minimum => "minimum",
            Self::Maximum => "maximum",
            Self::MinimumLength => "minimum_length",
            Self::MaximumLength => "maximum_length",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    field: Option<String>,
    rule: ValidationRule,
    code: Option<String>,
    message: String,
}

impl ValidationIssue {
    pub fn custom(message: impl Into<String>) -> Self {
        Self {
            field: None,
            rule: ValidationRule::Custom,
            code: None,
            message: message.into(),
        }
    }

    pub fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: None,
            rule: ValidationRule::Custom,
            code: Some(code.into()),
            message: message.into(),
        }
    }

    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    pub fn rule(&self) -> ValidationRule {
        self.rule
    }

    pub fn code(&self) -> &str {
        self.code.as_deref().unwrap_or(self.rule.as_str())
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
            code: None,
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

    #[doc(hidden)]
    pub fn prefix_field(mut self, prefix: &str) -> Self {
        self.field = Some(match self.field {
            Some(field) => format!("{prefix}.{field}"),
            None => prefix.to_owned(),
        });
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

    fn extend_nested(&mut self, prefix: &str, errors: Self) {
        self.issues.extend(
            errors
                .issues
                .into_iter()
                .map(|issue| issue.prefix_field(prefix)),
        );
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
        crate::HttpError::new(
            400,
            "request.validation.invalid",
            "Request validation failed",
        )
        .validation(self)
        .into_response()
    }
}

pub trait ValueSchema: Sized {
    fn decode_value(bytes: &[u8]) -> Result<Self, String>;

    fn metadata() -> SchemaMetadata {
        SchemaMetadata::new(SchemaKind::String)
    }
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

    fn metadata() -> SchemaMetadata {
        SchemaMetadata::new(SchemaKind::Bytes)
    }
}

macro_rules! value_schema {
    ($kind:ident: $($type:ty),+ $(,)?) => {
        $(
            impl ValueSchema for $type {
                fn decode_value(bytes: &[u8]) -> Result<Self, String> {
                    let value = std::str::from_utf8(bytes)
                        .map_err(|_| "must be valid UTF-8".to_owned())?;

                    value
                        .parse::<Self>()
                        .map_err(|_| format!("must be a valid {}", stringify!($type)))
                }

                fn metadata() -> SchemaMetadata {
                    SchemaMetadata::new(SchemaKind::$kind)
                }
            }
        )+
    };
}

value_schema!(Boolean: bool);
value_schema!(Integer: u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
value_schema!(Number: f32, f64);

impl ValueSchema for net::Ipv4Addr {
    fn decode_value(bytes: &[u8]) -> Result<Self, String> {
        decode_from_str(bytes, "IPv4 address")
    }

    fn metadata() -> SchemaMetadata {
        SchemaMetadata::new(SchemaKind::String).format("ipv4")
    }
}

impl ValueSchema for net::Ipv6Addr {
    fn decode_value(bytes: &[u8]) -> Result<Self, String> {
        decode_from_str(bytes, "IPv6 address")
    }

    fn metadata() -> SchemaMetadata {
        SchemaMetadata::new(SchemaKind::String).format("ipv6")
    }
}

impl ValueSchema for net::IpAddr {
    fn decode_value(bytes: &[u8]) -> Result<Self, String> {
        decode_from_str(bytes, "IP address")
    }

    fn metadata() -> SchemaMetadata {
        SchemaMetadata::new(SchemaKind::OneOf(vec![
            <net::Ipv4Addr as ValueSchema>::metadata(),
            <net::Ipv6Addr as ValueSchema>::metadata(),
        ]))
    }
}

fn decode_from_str<T: std::str::FromStr>(bytes: &[u8], expected: &str) -> Result<T, String> {
    let value = std::str::from_utf8(bytes).map_err(|_| "must be valid UTF-8".to_owned())?;
    value
        .parse()
        .map_err(|_| format!("must be a valid {expected}"))
}

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

    fn metadata() -> SchemaMetadata {
        T::metadata()
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

    pub fn required_nested<T: Schema>(&mut self, name: &str) -> Option<T> {
        let options = nested_options::<T>(self.options);
        let values = self.nested_values(name);

        if values.is_empty() {
            self.issue(Some(name), ValidationRule::Missing, "is required");
            return None;
        }

        match T::decode(&values, options) {
            Ok(value) => Some(value),
            Err(errors) => {
                self.errors.extend_nested(name, errors);
                None
            }
        }
    }

    pub fn optional_nested<T: Schema>(&mut self, name: &str) -> Option<Option<T>> {
        let options = nested_options::<T>(self.options);
        let values = self.nested_values(name);

        if values.is_empty() {
            return Some(None);
        }

        match T::decode(&values, options) {
            Ok(value) => Some(Some(value)),
            Err(errors) => {
                self.errors.extend_nested(name, errors);
                None
            }
        }
    }

    pub fn defaulted_nested<T: Schema, F: FnOnce() -> T>(
        &mut self,
        name: &str,
        default: F,
    ) -> Option<T> {
        let options = nested_options::<T>(self.options);
        let values = self.nested_values(name);

        if values.is_empty() {
            return Some(default());
        }

        match T::decode(&values, options) {
            Ok(value) => Some(value),
            Err(errors) => {
                self.errors.extend_nested(name, errors);
                None
            }
        }
    }

    pub fn repeated_nested<T: Schema>(&mut self, name: &str) -> Option<Vec<T>> {
        let prefix = format!("{name}.");
        let mut indexes = BTreeSet::new();
        let mut valid = true;

        for index in 0..self.values.len() {
            let Some(value) = self.values.value(index) else {
                continue;
            };
            let Some(remainder) = self.values.strip_name_prefix(value.name, &prefix) else {
                continue;
            };
            let Some((item, field)) = remainder.split_once('.') else {
                self.consumed[index] = true;
                self.issue(
                    Some(value.name),
                    ValidationRule::InvalidType,
                    "must use `<field>.<index>.<nested-field>`",
                );
                valid = false;
                continue;
            };

            if field.is_empty() {
                self.consumed[index] = true;
                self.issue(
                    Some(value.name),
                    ValidationRule::InvalidType,
                    "nested field name cannot be empty",
                );
                valid = false;
                continue;
            }

            match item.parse::<usize>() {
                Ok(item) => {
                    indexes.insert(item);
                }
                Err(_) => {
                    self.consumed[index] = true;
                    self.issue(
                        Some(value.name),
                        ValidationRule::InvalidType,
                        "nested item index must be a non-negative integer",
                    );
                    valid = false;
                }
            }
        }

        let mut decoded = Vec::with_capacity(indexes.len());
        for index in indexes {
            let item_name = format!("{name}.{index}");
            let options = nested_options::<T>(self.options);
            let values = self.nested_values(&item_name);

            match T::decode(&values, options) {
                Ok(value) => decoded.push(value),
                Err(errors) => {
                    self.errors.extend_nested(&item_name, errors);
                    valid = false;
                }
            }
        }

        valid.then_some(decoded)
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

    pub fn rest(&mut self) -> ExtraFields {
        let mut entries = Vec::new();

        for index in 0..self.values.len() {
            if self.consumed[index] {
                continue;
            }

            let Some(value) = self.values.value(index) else {
                continue;
            };

            entries.push((value.name.to_owned(), value.bytes.to_vec()));
            self.consumed[index] = true;
        }

        ExtraFields {
            entries,
            case_insensitive: self.values.names_are_case_insensitive(),
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

                if unknown
                    .iter()
                    .any(|name| self.values.name_matches(name, value.name))
                {
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

    fn nested_values(&mut self, name: &str) -> NestedValues<'_> {
        let prefix = format!("{name}.");
        let mut values = Vec::new();

        for index in 0..self.values.len() {
            let Some(value) = self.values.value(index) else {
                continue;
            };
            let Some(name) = self.values.strip_name_prefix(value.name, &prefix) else {
                continue;
            };

            self.consumed[index] = true;
            values.push(Value {
                name,
                bytes: value.bytes,
            });
        }

        NestedValues {
            values,
            case_insensitive: self.values.names_are_case_insensitive(),
        }
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

fn nested_options<T: Schema>(parent: DecodeOptions) -> DecodeOptions {
    DecodeOptions::new(T::UNKNOWN_FIELDS.unwrap_or(parent.unknown_fields()))
}

struct NestedValues<'values> {
    values: Vec<Value<'values>>,
    case_insensitive: bool,
}

impl Values for NestedValues<'_> {
    fn len(&self) -> usize {
        self.values.len()
    }

    fn value(&self, index: usize) -> Option<Value<'_>> {
        self.values.get(index).copied()
    }

    fn name_matches(&self, actual: &str, expected: &str) -> bool {
        if self.case_insensitive {
            actual.eq_ignore_ascii_case(expected)
        } else {
            actual == expected
        }
    }

    fn names_are_case_insensitive(&self) -> bool {
        self.case_insensitive
    }

    fn strip_name_prefix<'name>(&self, actual: &'name str, prefix: &str) -> Option<&'name str> {
        if self.case_insensitive
            && actual
                .get(..prefix.len())
                .is_some_and(|actual| actual.eq_ignore_ascii_case(prefix))
        {
            actual.get(prefix.len()..)
        } else {
            actual.strip_prefix(prefix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodeOptions, Schema, SchemaKind, Value, ValueSchema, Values};

    struct TestValues<'value> {
        entries: Vec<(&'value str, &'value [u8])>,
    }

    impl Values for TestValues<'_> {
        fn len(&self) -> usize {
            self.entries.len()
        }

        fn value(&self, index: usize) -> Option<Value<'_>> {
            self.entries
                .get(index)
                .map(|(name, bytes)| Value { name, bytes })
        }
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct Filter {
        name: String,
        minimum: u32,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct Search {
        #[schema(nested)]
        filter: Filter,
        #[schema(nested)]
        paging: Option<Paging>,
    }

    #[derive(Debug, Default, PartialEq, crate::Schema)]
    struct Paging {
        page: u32,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct NestedCollection {
        #[schema(nested)]
        filters: Vec<Filter>,
        #[schema(nested, default)]
        paging: Paging,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    #[schema(tag = "type", rename_all = "snake_case")]
    enum Selection {
        All,
        Range { start: u32, end: u32 },
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct Formatted {
        address: std::net::IpAddr,
        #[schema(format = "uuid")]
        identifier: String,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    #[schema(rename_all = "kebab-case")]
    enum Mode {
        FastMode,
        #[schema(rename = "safe")]
        SafeMode,
    }

    #[derive(Debug, PartialEq, crate::Schema)]
    struct Wrapper<T> {
        value: T,
    }

    #[derive(Debug, PartialEq)]
    struct Identifier(u64);

    impl ValueSchema for Identifier {
        fn decode_value(bytes: &[u8]) -> Result<Self, String> {
            let value = std::str::from_utf8(bytes)
                .map_err(|_| "must be valid UTF-8".to_owned())?
                .parse()
                .map_err(|_| "must be an identifier".to_owned())?;
            Ok(Self(value))
        }
    }

    #[test]
    fn decodes_nested_fields_from_dotted_names() {
        let values = TestValues {
            entries: vec![("filter.name", b"gpu"), ("filter.minimum", b"4")],
        };
        let decoded = Search::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.filter.name, "gpu");
        assert_eq!(decoded.filter.minimum, 4);
        assert_eq!(decoded.paging, None);
    }

    #[test]
    fn derives_string_enums_with_rename_rules() {
        let fast = TestValues {
            entries: vec![("mode", b"fast-mode")],
        };
        let safe = TestValues {
            entries: vec![("mode", b"safe")],
        };

        assert_eq!(
            Mode::decode(&fast, DecodeOptions::reject_unknown()).unwrap(),
            Mode::FastMode,
        );
        assert_eq!(
            Mode::decode(&safe, DecodeOptions::reject_unknown()).unwrap(),
            Mode::SafeMode,
        );
    }

    #[test]
    fn derives_generic_schemas() {
        let values = TestValues {
            entries: vec![("value", b"42")],
        };
        let decoded = Wrapper::<u64>::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.value, 42);
    }

    #[test]
    fn accepts_custom_value_schemas() {
        let values = TestValues {
            entries: vec![("value", b"91")],
        };
        let decoded =
            Wrapper::<Identifier>::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.value, Identifier(91));
    }

    #[test]
    fn exposes_nested_openapi_metadata() {
        let metadata = Search::metadata();
        let SchemaKind::Object(fields) = metadata.kind() else {
            panic!("expected object metadata");
        };

        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name(), "filter");
        assert!(fields[0].required());
        assert!(!fields[1].required());
        assert!(matches!(fields[0].schema().kind(), SchemaKind::Object(_)));
    }

    #[test]
    fn decodes_repeated_nested_fields_and_nested_defaults() {
        let values = TestValues {
            entries: vec![
                ("filters.0.name", b"gpu"),
                ("filters.0.minimum", b"4"),
                ("filters.1.name", b"cpu"),
                ("filters.1.minimum", b"8"),
            ],
        };
        let decoded = NestedCollection::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.filters.len(), 2);
        assert_eq!(decoded.filters[0].name, "gpu");
        assert_eq!(decoded.filters[1].minimum, 8);
        assert_eq!(decoded.paging, Paging::default());
    }

    #[test]
    fn decodes_tagged_enums_with_named_data() {
        let range = TestValues {
            entries: vec![("type", b"range"), ("start", b"2"), ("end", b"9")],
        };
        let all = TestValues {
            entries: vec![("type", b"all")],
        };

        assert_eq!(
            Selection::decode(&range, DecodeOptions::reject_unknown()).unwrap(),
            Selection::Range { start: 2, end: 9 },
        );
        assert_eq!(
            Selection::decode(&all, DecodeOptions::reject_unknown()).unwrap(),
            Selection::All,
        );

        let metadata = Selection::metadata();
        assert_eq!(metadata.discriminator_property(), Some("type"));
        assert!(matches!(metadata.kind(), SchemaKind::OneOf(variants) if variants.len() == 2));
    }

    #[test]
    fn exposes_formats_and_decodes_standard_ip_types() {
        let values = TestValues {
            entries: vec![("address", b"127.0.0.1"), ("identifier", b"abc")],
        };
        let decoded = Formatted::decode(&values, DecodeOptions::reject_unknown()).unwrap();

        assert_eq!(decoded.address, std::net::Ipv4Addr::LOCALHOST);
        let SchemaKind::Object(fields) = Formatted::metadata().kind().clone() else {
            panic!("expected object metadata");
        };
        assert_eq!(fields[1].schema().format_value(), Some("uuid"));
    }
}
