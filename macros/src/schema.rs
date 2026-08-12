use std::{fmt::Write, str::FromStr};

use proc_macro::{Delimiter, Group, TokenStream, TokenTree};

pub(crate) fn expand(input: TokenStream) -> Result<TokenStream, String> {
    let schema = StructSchema::parse(input)?;
    TokenStream::from_str(&schema.implementation()?).map_err(|error| error.to_string())
}

struct StructSchema {
    name: String,
    fields: Vec<Field>,
    validate: Option<String>,
}

impl StructSchema {
    fn parse(input: TokenStream) -> Result<Self, String> {
        let tokens = input.into_iter().collect::<Vec<_>>();
        let struct_index = tokens
            .iter()
            .position(|token| is_ident(token, "struct"))
            .ok_or_else(|| "Schema can only be derived for a struct".to_owned())?;
        let attributes = Attributes::parse(&tokens[..struct_index])?;
        let name = match tokens.get(struct_index + 1) {
            Some(TokenTree::Ident(name)) => name.to_string(),
            _ => return Err("expected a struct name".to_owned()),
        };
        let body_index = tokens[struct_index + 2..]
            .iter()
            .position(|token| matches!(token, TokenTree::Group(group) if group.delimiter() == Delimiter::Brace))
            .map(|index| index + struct_index + 2)
            .ok_or_else(|| "Schema requires a struct with named fields".to_owned())?;

        if body_index != struct_index + 2 {
            return Err("generic Schema structs are not supported yet".to_owned());
        }

        let body = match &tokens[body_index] {
            TokenTree::Group(group) => group.stream(),
            _ => unreachable!(),
        };
        let rename_all = match attributes.rename_all.as_deref() {
            None => RenameAll::None,
            Some("kebab-case") => RenameAll::KebabCase,
            Some(other) => return Err(format!("unsupported rename_all rule `{other}`")),
        };
        let fields = split_fields(body)
            .into_iter()
            .map(|tokens| Field::parse(tokens, rename_all))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            name,
            fields,
            validate: attributes.validate,
        })
    }

    fn implementation(&self) -> Result<String, String> {
        let mut output = format!(
            "impl ::serverkit::schemaval::Schema for {} {{\
                fn decode<__SchemavalValues: ::serverkit::schemaval::Values>(\
                    __schemaval_values: &__SchemavalValues,\
                    __schemaval_options: ::serverkit::schemaval::DecodeOptions,\
                ) -> ::core::result::Result<Self, ::serverkit::schemaval::ValidationErrors> {{\
                    let mut __schemaval_decoder = ::serverkit::schemaval::Decoder::new(\
                        __schemaval_values,\
                        __schemaval_options,\
                    );",
            self.name,
        );

        for (index, field) in self.fields.iter().enumerate() {
            field.write_decode(index, &mut output)?;
        }

        output.push_str("let __schemaval_errors = __schemaval_decoder.finish();");

        if self.fields.is_empty() {
            output.push_str(
                "if !__schemaval_errors.is_empty() {\
                    return ::core::result::Result::Err(__schemaval_errors);\
                }\
                let __schemaval_value = Self {};",
            );
            self.write_struct_validation(&mut output);
            output.push_str("::core::result::Result::Ok(__schemaval_value)}}}");
            return Ok(output);
        }

        output.push_str("match (");
        for index in 0..self.fields.len() {
            write!(output, "__schemaval_field_{index},").map_err(|error| error.to_string())?;
        }
        output.push_str(") {");
        output.push('(');
        for index in 0..self.fields.len() {
            write!(
                output,
                "::core::option::Option::Some(__schemaval_field_{index}),"
            )
            .map_err(|error| error.to_string())?;
        }
        output.push_str(") if __schemaval_errors.is_empty() => {");
        output.push_str("let __schemaval_value = Self {");
        for (index, field) in self.fields.iter().enumerate() {
            write!(output, "{}: __schemaval_field_{index},", field.rust_name)
                .map_err(|error| error.to_string())?;
        }
        output.push_str("};");
        self.write_struct_validation(&mut output);
        output.push_str(
            "::core::result::Result::Ok(__schemaval_value)\
                }\
                _ => ::core::result::Result::Err(__schemaval_errors),\
            }\
        }\
    }",
        );

        Ok(output)
    }

    fn write_struct_validation(&self, output: &mut String) {
        if let Some(validate) = &self.validate {
            write!(
                output,
                "if let ::core::result::Result::Err(__schemaval_issue) = \
                    {validate}(&__schemaval_value) {{\
                        return ::core::result::Result::Err(\
                            ::serverkit::schemaval::ValidationErrors::from_issue(\
                                __schemaval_issue,\
                            ),\
                        );\
                    }}",
            )
            .expect("writing to a String cannot fail");
        }
    }
}

#[derive(Clone, Copy)]
enum RenameAll {
    None,
    KebabCase,
}

impl RenameAll {
    fn apply(self, name: &str) -> String {
        match self {
            Self::None => name.to_owned(),
            Self::KebabCase => name.replace('_', "-"),
        }
    }
}

struct Field {
    rust_name: String,
    schema_name: String,
    type_name: String,
    kind: FieldKind,
    default: Option<DefaultValue>,
    minimum: Option<String>,
    maximum: Option<String>,
    minimum_length: Option<String>,
    maximum_length: Option<String>,
    validate: Option<String>,
}

impl Field {
    fn parse(tokens: Vec<TokenTree>, rename_all: RenameAll) -> Result<Self, String> {
        let colon = tokens
            .iter()
            .position(|token| is_punct(token, ':'))
            .ok_or_else(|| "expected a named field".to_owned())?;
        let rust_name = tokens[..colon]
            .iter()
            .rev()
            .find_map(|token| match token {
                TokenTree::Ident(name) => Some(name.to_string()),
                _ => None,
            })
            .ok_or_else(|| "expected a field name".to_owned())?;
        let attributes = Attributes::parse(&tokens[..colon])?;
        let type_tokens = tokens[colon + 1..].to_vec();

        if type_tokens.is_empty() {
            return Err(format!("field `{rust_name}` is missing a type"));
        }

        let type_name = TokenStream::from_iter(type_tokens.iter().cloned()).to_string();
        let kind = FieldKind::from_type(&type_tokens);

        if attributes.default.is_some() && !matches!(kind, FieldKind::Required) {
            return Err(format!(
                "field `{rust_name}` cannot combine default with Option or repeated Vec"
            ));
        }

        Ok(Self {
            schema_name: attributes
                .rename
                .unwrap_or_else(|| rename_all.apply(&rust_name)),
            rust_name,
            type_name,
            kind,
            default: attributes.default,
            minimum: attributes.minimum,
            maximum: attributes.maximum,
            minimum_length: attributes.minimum_length,
            maximum_length: attributes.maximum_length,
            validate: attributes.validate,
        })
    }

    fn write_decode(&self, index: usize, output: &mut String) -> Result<(), String> {
        let name = string_literal(&self.schema_name);
        let decode = match (&self.kind, &self.default) {
            (FieldKind::Required, None) => {
                format!("__schemaval_decoder.required::<{}>({name})", self.type_name,)
            }
            (FieldKind::Required, Some(DefaultValue::Default)) => format!(
                "__schemaval_decoder.defaulted::<{}, _>({name}, \
                    || ::core::default::Default::default())",
                self.type_name,
            ),
            (FieldKind::Required, Some(DefaultValue::Expression(expression))) => format!(
                "__schemaval_decoder.defaulted::<{}, _>({name}, || ({expression}))",
                self.type_name,
            ),
            (FieldKind::Optional(inner), None) => {
                format!("__schemaval_decoder.optional::<{inner}>({name})")
            }
            (FieldKind::Repeated(inner), None) => {
                format!("__schemaval_decoder.repeated::<{inner}>({name})")
            }
            _ => {
                return Err(format!(
                    "invalid field configuration for `{}`",
                    self.rust_name
                ));
            }
        };

        write!(output, "let __schemaval_field_{index} = {decode};")
            .map_err(|error| error.to_string())?;

        let constraint_value = match self.kind {
            FieldKind::Optional(_) => format!(
                "__schemaval_field_{index}.as_ref().and_then(\
                    |__schemaval_value| __schemaval_value.as_ref()\
                )"
            ),
            _ => format!("__schemaval_field_{index}.as_ref()"),
        };

        if self.minimum.is_some()
            || self.maximum.is_some()
            || self.minimum_length.is_some()
            || self.maximum_length.is_some()
        {
            write!(
                output,
                "if let ::core::option::Option::Some(__schemaval_value) = {constraint_value} {{"
            )
            .map_err(|error| error.to_string())?;

            if let Some(minimum) = &self.minimum {
                write!(
                    output,
                    "__schemaval_decoder.minimum({name}, __schemaval_value, ({minimum}));"
                )
                .map_err(|error| error.to_string())?;
            }
            if let Some(maximum) = &self.maximum {
                write!(
                    output,
                    "__schemaval_decoder.maximum({name}, __schemaval_value, ({maximum}));"
                )
                .map_err(|error| error.to_string())?;
            }
            if let Some(minimum) = &self.minimum_length {
                write!(
                    output,
                    "__schemaval_decoder.minimum_length(\
                        {name}, __schemaval_value, ({minimum}) as usize\
                    );"
                )
                .map_err(|error| error.to_string())?;
            }
            if let Some(maximum) = &self.maximum_length {
                write!(
                    output,
                    "__schemaval_decoder.maximum_length(\
                        {name}, __schemaval_value, ({maximum}) as usize\
                    );"
                )
                .map_err(|error| error.to_string())?;
            }

            output.push('}');
        }

        if let Some(validate) = &self.validate {
            write!(
                output,
                "if let ::core::option::Option::Some(__schemaval_value) = \
                    __schemaval_field_{index}.as_ref() {{\
                        __schemaval_decoder.custom(\
                            {name},\
                            {validate}(__schemaval_value),\
                        );\
                    }}",
            )
            .map_err(|error| error.to_string())?;
        }

        Ok(())
    }
}

enum FieldKind {
    Required,
    Optional(String),
    Repeated(String),
}

impl FieldKind {
    fn from_type(tokens: &[TokenTree]) -> Self {
        let Some((wrapper, inner)) = generic_wrapper(tokens) else {
            return Self::Required;
        };

        match wrapper.as_str() {
            "Option" => Self::Optional(inner),
            "Vec" if inner != "u8" => Self::Repeated(inner),
            _ => Self::Required,
        }
    }
}

enum DefaultValue {
    Default,
    Expression(String),
}

#[derive(Default)]
struct Attributes {
    rename: Option<String>,
    rename_all: Option<String>,
    default: Option<DefaultValue>,
    minimum: Option<String>,
    maximum: Option<String>,
    minimum_length: Option<String>,
    maximum_length: Option<String>,
    validate: Option<String>,
}

impl Attributes {
    fn parse(tokens: &[TokenTree]) -> Result<Self, String> {
        let mut attributes = Self::default();
        let mut index = 0;

        while index + 1 < tokens.len() {
            if !is_punct(&tokens[index], '#') {
                index += 1;
                continue;
            }

            let TokenTree::Group(group) = &tokens[index + 1] else {
                index += 1;
                continue;
            };

            if group.delimiter() != Delimiter::Bracket {
                index += 1;
                continue;
            }

            attributes.parse_attribute(group)?;
            index += 2;
        }

        Ok(attributes)
    }

    fn parse_attribute(&mut self, attribute: &Group) -> Result<(), String> {
        let tokens = attribute.stream().into_iter().collect::<Vec<_>>();

        if !tokens
            .first()
            .is_some_and(|token| is_ident(token, "schema"))
        {
            return Ok(());
        }

        let Some(TokenTree::Group(arguments)) = tokens.get(1) else {
            return Err("schema attributes require parentheses".to_owned());
        };

        if arguments.delimiter() != Delimiter::Parenthesis {
            return Err("schema attributes require parentheses".to_owned());
        }

        for item in split_commas(arguments.stream()) {
            self.parse_item(item)?;
        }

        Ok(())
    }

    fn parse_item(&mut self, item: Vec<TokenTree>) -> Result<(), String> {
        let key = match item.first() {
            Some(TokenTree::Ident(key)) => key.to_string(),
            _ => return Err("expected a schema attribute name".to_owned()),
        };

        if key == "default" && item.len() == 1 {
            self.default = Some(DefaultValue::Default);
            return Ok(());
        }

        if !item.get(1).is_some_and(|token| is_punct(token, '=')) {
            return Err(format!("schema attribute `{key}` requires a value"));
        }

        let value = item[2..].to_vec();
        if value.is_empty() {
            return Err(format!("schema attribute `{key}` requires a value"));
        }
        let expression = TokenStream::from_iter(value.iter().cloned()).to_string();

        match key.as_str() {
            "rename" => self.rename = Some(parse_string(&value)?),
            "rename_all" => self.rename_all = Some(parse_string(&value)?),
            "default" => self.default = Some(DefaultValue::Expression(expression)),
            "minimum" => self.minimum = Some(expression),
            "maximum" => self.maximum = Some(expression),
            "min_length" => self.minimum_length = Some(expression),
            "max_length" => self.maximum_length = Some(expression),
            "validate" => self.validate = Some(expression),
            _ => return Err(format!("unknown schema attribute `{key}`")),
        }

        Ok(())
    }
}

fn split_fields(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    split_commas(stream)
        .into_iter()
        .filter(|field| !field.is_empty())
        .collect()
}

fn split_commas(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut items = Vec::new();
    let mut item = Vec::new();
    let mut angles = 0usize;

    for token in stream {
        if is_punct(&token, '<') {
            angles += 1;
        } else if is_punct(&token, '>') {
            angles = angles.saturating_sub(1);
        }

        if angles == 0 && is_punct(&token, ',') {
            items.push(item);
            item = Vec::new();
        } else {
            item.push(token);
        }
    }

    if !item.is_empty() {
        items.push(item);
    }

    items
}

fn generic_wrapper(tokens: &[TokenTree]) -> Option<(String, String)> {
    let opening = tokens.iter().position(|token| is_punct(token, '<'))?;
    let closing = tokens.iter().rposition(|token| is_punct(token, '>'))?;

    if closing != tokens.len() - 1 || opening >= closing {
        return None;
    }

    let wrapper = tokens[..opening]
        .iter()
        .rev()
        .find_map(|token| match token {
            TokenTree::Ident(name) => Some(name.to_string()),
            _ => None,
        })?;
    let inner = TokenStream::from_iter(tokens[opening + 1..closing].iter().cloned()).to_string();

    Some((wrapper, inner))
}

fn parse_string(tokens: &[TokenTree]) -> Result<String, String> {
    let [TokenTree::Literal(literal)] = tokens else {
        return Err("expected a string literal".to_owned());
    };
    let literal = literal.to_string();

    if !literal.starts_with('"') || !literal.ends_with('"') {
        return Err("expected a string literal".to_owned());
    }

    let value = &literal[1..literal.len() - 1];
    if value.contains('\\') {
        return Err("escaped schema names are not supported".to_owned());
    }

    Ok(value.to_owned())
}

fn string_literal(value: &str) -> String {
    format!("{value:?}")
}

fn is_ident(token: &TokenTree, expected: &str) -> bool {
    matches!(token, TokenTree::Ident(ident) if ident.to_string() == expected)
}

fn is_punct(token: &TokenTree, expected: char) -> bool {
    matches!(token, TokenTree::Punct(punct) if punct.as_char() == expected)
}
