use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, Field, Fields, GenericArgument,
    Generics, LitStr, PathArguments, Type, Variant, parse_quote,
};

pub(crate) fn expand(input: TokenStream) -> Result<TokenStream, String> {
    let input = syn::parse::<DeriveInput>(input).map_err(|error| error.to_string())?;
    let output = match &input.data {
        Data::Struct(data) => expand_struct(&input, data),
        Data::Enum(data) => expand_enum(&input, data),
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input,
            "Schema cannot be derived for a union",
        )),
    }
    .map_err(|error| error.to_string())?;

    Ok(output.into())
}

fn expand_struct(input: &DeriveInput, data: &DataStruct) -> syn::Result<TokenStream2> {
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "Schema requires a struct with named fields",
        ));
    };
    let attributes = Attributes::parse(&input.attrs)?;
    attributes.validate_for_struct()?;
    let rename_all = RenameAll::parse(attributes.rename_all.as_deref())?;
    let fields = fields
        .named
        .iter()
        .map(|field| FieldSchema::parse(field, rename_all))
        .collect::<syn::Result<Vec<_>>>()?;
    let rest_count = fields.iter().filter(|field| field.rest).count();

    if rest_count > 1 {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "Schema can contain only one `rest` field",
        ));
    }

    if rest_count == 1 && attributes.unknown_fields.is_some() {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "`unknown_fields` cannot be combined with a `rest` field",
        ));
    }

    let mut generics = input.generics.clone();
    add_field_bounds(&mut generics, &fields);
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let name = &input.ident;
    let unknown_fields = match attributes.unknown_fields.as_deref() {
        None => quote!(::core::option::Option::None),
        Some("reject") => quote!(::core::option::Option::Some(
            ::serverkit::schemaval::UnknownFields::Reject
        )),
        Some("ignore") => quote!(::core::option::Option::Some(
            ::serverkit::schemaval::UnknownFields::Ignore
        )),
        Some(value) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                format!(
                    "unsupported unknown_fields policy `{value}`; expected `reject` or `ignore`"
                ),
            ));
        }
    };
    let decodes = fields
        .iter()
        .enumerate()
        .filter(|(_, field)| !field.rest)
        .chain(fields.iter().enumerate().filter(|(_, field)| field.rest))
        .map(|(index, field)| field.decode(index))
        .collect::<syn::Result<Vec<_>>>()?;
    let variables = (0..fields.len())
        .map(|index| format_ident!("__schemaval_field_{index}"))
        .collect::<Vec<_>>();
    let field_names = fields.iter().map(|field| &field.ident).collect::<Vec<_>>();
    let construct = if fields.is_empty() {
        quote! {
            if !__schemaval_errors.is_empty() {
                return ::core::result::Result::Err(__schemaval_errors);
            }
            let __schemaval_value = Self {};
        }
    } else {
        quote! {
            let __schemaval_value = match (#(#variables,)*) {
                (#(::core::option::Option::Some(#variables),)*)
                    if __schemaval_errors.is_empty() => Self {
                        #(#field_names: #variables,)*
                    },
                _ => return ::core::result::Result::Err(__schemaval_errors),
            };
        }
    };
    let struct_validation = attributes.validate.map(|validate| {
        quote! {
            if let ::core::result::Result::Err(__schemaval_issue) =
                #validate(&__schemaval_value)
            {
                return ::core::result::Result::Err(
                    ::serverkit::schemaval::ValidationErrors::from_issue(
                        __schemaval_issue,
                    ),
                );
            }
        }
    });
    let metadata_fields = fields
        .iter()
        .filter(|field| !field.rest)
        .map(FieldSchema::metadata)
        .collect::<Vec<_>>();

    Ok(quote! {
        impl #impl_generics ::serverkit::schemaval::Schema
            for #name #type_generics #where_clause
        {
            const UNKNOWN_FIELDS: ::core::option::Option<
                ::serverkit::schemaval::UnknownFields
            > = #unknown_fields;

            fn decode<__SchemavalValues: ::serverkit::schemaval::Values>(
                __schemaval_values: &__SchemavalValues,
                __schemaval_options: ::serverkit::schemaval::DecodeOptions,
            ) -> ::core::result::Result<
                Self,
                ::serverkit::schemaval::ValidationErrors,
            > {
                let mut __schemaval_decoder = ::serverkit::schemaval::Decoder::new(
                    __schemaval_values,
                    __schemaval_options,
                );
                #(#decodes)*
                let __schemaval_errors = __schemaval_decoder.finish();
                #construct
                #struct_validation
                ::core::result::Result::Ok(__schemaval_value)
            }

            fn metadata() -> ::serverkit::schemaval::SchemaMetadata {
                ::serverkit::schemaval::SchemaMetadata::named(
                    ::core::any::type_name::<Self>(),
                    ::serverkit::schemaval::SchemaKind::Object(
                        ::std::vec![#(#metadata_fields),*],
                    ),
                )
            }
        }
    })
}

fn expand_enum(input: &DeriveInput, data: &DataEnum) -> syn::Result<TokenStream2> {
    let attributes = Attributes::parse(&input.attrs)?;
    attributes.validate_for_enum()?;
    let rename_all = RenameAll::parse(attributes.rename_all.as_deref())?;
    let variants = data
        .variants
        .iter()
        .map(|variant| EnumVariant::parse(variant, rename_all))
        .collect::<syn::Result<Vec<_>>>()?;
    let generics = input.generics.clone();
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let name = &input.ident;
    let matches = variants.iter().map(|variant| {
        let value = &variant.value;
        let ident = &variant.ident;
        quote!(#value => ::core::result::Result::Ok(Self::#ident),)
    });
    let values = variants.iter().map(|variant| &variant.value);

    Ok(quote! {
        impl #impl_generics ::serverkit::schemaval::ValueSchema
            for #name #type_generics #where_clause
        {
            fn decode_value(bytes: &[u8]) -> ::core::result::Result<Self, ::std::string::String> {
                let value = ::core::str::from_utf8(bytes)
                    .map_err(|_| "must be valid UTF-8".to_owned())?;

                match value {
                    #(#matches)*
                    _ => ::core::result::Result::Err(
                        "must be one of the declared enum values".to_owned(),
                    ),
                }
            }

            fn metadata() -> ::serverkit::schemaval::SchemaMetadata {
                ::serverkit::schemaval::SchemaMetadata::named(
                    ::core::any::type_name::<Self>(),
                    ::serverkit::schemaval::SchemaKind::Enum(
                        ::std::vec![#(#values.to_owned()),*],
                    ),
                )
            }
        }
    })
}

fn add_field_bounds(generics: &mut Generics, fields: &[FieldSchema]) {
    let where_clause = generics.make_where_clause();

    for field in fields.iter().filter(|field| !field.rest) {
        let value_type = &field.value_type;
        let predicate = if field.nested {
            parse_quote!(#value_type: ::serverkit::schemaval::Schema)
        } else {
            parse_quote!(#value_type: ::serverkit::schemaval::ValueSchema)
        };
        where_clause.predicates.push(predicate);
    }
}

struct FieldSchema {
    ident: syn::Ident,
    input_name: String,
    field_type: Type,
    value_type: Type,
    kind: FieldKind,
    rest: bool,
    nested: bool,
    default: Option<DefaultValue>,
    minimum: Option<Expr>,
    maximum: Option<Expr>,
    minimum_length: Option<Expr>,
    maximum_length: Option<Expr>,
    validate: Option<Expr>,
}

impl FieldSchema {
    fn parse(field: &Field, rename_all: RenameAll) -> syn::Result<Self> {
        let ident = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new_spanned(field, "expected a named field"))?;
        let attributes = Attributes::parse(&field.attrs)?;
        attributes.validate_for_field(&ident)?;
        let kind = FieldKind::from_type(&field.ty);
        let value_type = kind.value_type(&field.ty).clone();
        let rest = attributes.rest;

        if rest && !is_type_named(&field.ty, "ExtraFields") {
            return Err(syn::Error::new_spanned(
                &field.ty,
                format!("rest field `{ident}` must have type `ExtraFields`"),
            ));
        }

        if attributes.nested && matches!(kind, FieldKind::Repeated) {
            return Err(syn::Error::new_spanned(
                &field.ty,
                "repeated nested fields are not supported",
            ));
        }

        if attributes.nested && attributes.default.is_some() {
            return Err(syn::Error::new_spanned(
                &field.ty,
                "nested fields cannot use `default`",
            ));
        }

        if attributes.default.is_some() && !matches!(kind, FieldKind::Required) {
            return Err(syn::Error::new_spanned(
                &field.ty,
                format!("field `{ident}` cannot combine default with Option or repeated Vec"),
            ));
        }

        let input_name = attributes
            .rename
            .clone()
            .unwrap_or_else(|| rename_all.apply(&ident.to_string()));

        Ok(Self {
            ident,
            input_name,
            field_type: field.ty.clone(),
            value_type,
            kind,
            rest,
            nested: attributes.nested,
            default: attributes.default,
            minimum: attributes.minimum,
            maximum: attributes.maximum,
            minimum_length: attributes.minimum_length,
            maximum_length: attributes.maximum_length,
            validate: attributes.validate,
        })
    }

    fn decode(&self, index: usize) -> syn::Result<TokenStream2> {
        let variable = format_ident!("__schemaval_field_{index}");

        if self.rest {
            return Ok(quote! {
                let #variable = ::core::option::Option::Some(
                    __schemaval_decoder.rest(),
                );
            });
        }

        let name = &self.input_name;
        let value_type = &self.value_type;
        let field_type = &self.field_type;
        let decode = if self.nested {
            match self.kind {
                FieldKind::Required => quote!(
                    __schemaval_decoder.required_nested::<#value_type>(#name)
                ),
                FieldKind::Optional => quote!(
                    __schemaval_decoder.optional_nested::<#value_type>(#name)
                ),
                FieldKind::Repeated => unreachable!(),
            }
        } else {
            match (&self.kind, &self.default) {
                (FieldKind::Required, None) => quote!(
                    __schemaval_decoder.required::<#field_type>(#name)
                ),
                (FieldKind::Required, Some(DefaultValue::Default)) => quote!(
                    __schemaval_decoder.defaulted::<#field_type, _>(
                        #name,
                        || ::core::default::Default::default(),
                    )
                ),
                (FieldKind::Required, Some(DefaultValue::Expression(expression))) => quote!(
                    __schemaval_decoder.defaulted::<#field_type, _>(
                        #name,
                        || (#expression),
                    )
                ),
                (FieldKind::Optional, None) => quote!(
                    __schemaval_decoder.optional::<#value_type>(#name)
                ),
                (FieldKind::Repeated, None) => quote!(
                    __schemaval_decoder.repeated::<#value_type>(#name)
                ),
                _ => {
                    return Err(syn::Error::new_spanned(
                        &self.field_type,
                        format!("invalid field configuration for `{}`", self.ident),
                    ));
                }
            }
        };
        let constraint_value = match self.kind {
            FieldKind::Optional => quote!(
                #variable.as_ref().and_then(|value| value.as_ref())
            ),
            FieldKind::Required | FieldKind::Repeated => quote!(#variable.as_ref()),
        };
        let minimum = self.minimum.as_ref().map(|minimum| {
            quote! {
                __schemaval_decoder.minimum(#name, __schemaval_value, (#minimum));
            }
        });
        let maximum = self.maximum.as_ref().map(|maximum| {
            quote! {
                __schemaval_decoder.maximum(#name, __schemaval_value, (#maximum));
            }
        });
        let minimum_length = self.minimum_length.as_ref().map(|minimum| {
            quote! {
                __schemaval_decoder.minimum_length(
                    #name,
                    __schemaval_value,
                    (#minimum) as usize,
                );
            }
        });
        let maximum_length = self.maximum_length.as_ref().map(|maximum| {
            quote! {
                __schemaval_decoder.maximum_length(
                    #name,
                    __schemaval_value,
                    (#maximum) as usize,
                );
            }
        });
        let constraints = if self.minimum.is_some()
            || self.maximum.is_some()
            || self.minimum_length.is_some()
            || self.maximum_length.is_some()
        {
            quote! {
                if let ::core::option::Option::Some(__schemaval_value) = #constraint_value {
                    #minimum
                    #maximum
                    #minimum_length
                    #maximum_length
                }
            }
        } else {
            TokenStream2::new()
        };
        let validation = self.validate.as_ref().map(|validate| {
            quote! {
                if let ::core::option::Option::Some(__schemaval_value) = #constraint_value {
                    __schemaval_decoder.custom(
                        #name,
                        #validate(__schemaval_value),
                    );
                }
            }
        });

        Ok(quote! {
            let #variable = #decode;
            #constraints
            #validation
        })
    }

    fn metadata(&self) -> TokenStream2 {
        let name = &self.input_name;
        let value_type = &self.value_type;
        let required = matches!(self.kind, FieldKind::Required) && self.default.is_none();
        let base = if self.nested {
            quote!(<#value_type as ::serverkit::schemaval::Schema>::metadata())
        } else {
            quote!(<#value_type as ::serverkit::schemaval::ValueSchema>::metadata())
        };
        let schema = if matches!(self.kind, FieldKind::Repeated) {
            quote!(::serverkit::schemaval::SchemaMetadata::array(#base))
        } else {
            base
        };
        let minimum = self
            .minimum
            .as_ref()
            .map(|minimum| quote!(.minimum(#minimum)));
        let maximum = self
            .maximum
            .as_ref()
            .map(|maximum| quote!(.maximum(#maximum)));
        let minimum_length = self
            .minimum_length
            .as_ref()
            .map(|minimum| quote!(.minimum_length((#minimum) as usize)));
        let maximum_length = self
            .maximum_length
            .as_ref()
            .map(|maximum| quote!(.maximum_length((#maximum) as usize)));

        quote! {
            ::serverkit::schemaval::SchemaField::new(
                #name,
                #schema,
                #required,
            )
            #minimum
            #maximum
            #minimum_length
            #maximum_length
        }
    }
}

#[derive(Clone, Copy)]
enum FieldKind {
    Required,
    Optional,
    Repeated,
}

impl FieldKind {
    fn from_type(field_type: &Type) -> Self {
        if generic_inner(field_type, "Option").is_some() {
            Self::Optional
        } else if generic_inner(field_type, "Vec").is_some()
            && !generic_inner(field_type, "Vec").is_some_and(|inner| is_type_named(inner, "u8"))
        {
            Self::Repeated
        } else {
            Self::Required
        }
    }

    fn value_type<'field>(&self, field_type: &'field Type) -> &'field Type {
        match self {
            Self::Required => field_type,
            Self::Optional => generic_inner(field_type, "Option").unwrap_or(field_type),
            Self::Repeated => generic_inner(field_type, "Vec").unwrap_or(field_type),
        }
    }
}

struct EnumVariant {
    ident: syn::Ident,
    value: String,
}

impl EnumVariant {
    fn parse(variant: &Variant, rename_all: RenameAll) -> syn::Result<Self> {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "Schema enums can contain only unit variants",
            ));
        }

        let attributes = Attributes::parse(&variant.attrs)?;
        attributes.validate_for_variant(&variant.ident)?;
        let value = attributes
            .rename
            .unwrap_or_else(|| rename_all.apply(&variant.ident.to_string()));

        Ok(Self {
            ident: variant.ident.clone(),
            value,
        })
    }
}

#[derive(Default)]
struct Attributes {
    rename: Option<String>,
    rename_all: Option<String>,
    default: Option<DefaultValue>,
    minimum: Option<Expr>,
    maximum: Option<Expr>,
    minimum_length: Option<Expr>,
    maximum_length: Option<Expr>,
    validate: Option<Expr>,
    unknown_fields: Option<String>,
    rest: bool,
    nested: bool,
}

impl Attributes {
    fn parse(attributes: &[Attribute]) -> syn::Result<Self> {
        let mut parsed = Self::default();

        for attribute in attributes {
            if !attribute.path().is_ident("schema") {
                continue;
            }

            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("rest") {
                    parsed.rest = true;
                    return Ok(());
                }

                if meta.path.is_ident("nested") {
                    parsed.nested = true;
                    return Ok(());
                }

                if meta.path.is_ident("default") && !meta.input.peek(syn::Token![=]) {
                    parsed.default = Some(DefaultValue::Default);
                    return Ok(());
                }

                if meta.path.is_ident("rename") {
                    parsed.rename = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("rename_all") {
                    parsed.rename_all = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("default") {
                    parsed.default = Some(DefaultValue::Expression(meta.value()?.parse()?));
                } else if meta.path.is_ident("minimum") {
                    parsed.minimum = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("maximum") {
                    parsed.maximum = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("min_length") {
                    parsed.minimum_length = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("max_length") {
                    parsed.maximum_length = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("validate") {
                    parsed.validate = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("unknown_fields") {
                    parsed.unknown_fields = Some(meta.value()?.parse::<LitStr>()?.value());
                } else {
                    return Err(meta.error("unknown schema attribute"));
                }

                Ok(())
            })?;
        }

        Ok(parsed)
    }

    fn validate_for_struct(&self) -> syn::Result<()> {
        if self.rename.is_some()
            || self.default.is_some()
            || self.minimum.is_some()
            || self.maximum.is_some()
            || self.minimum_length.is_some()
            || self.maximum_length.is_some()
            || self.rest
            || self.nested
        {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "invalid struct-level schema attribute",
            ));
        }
        Ok(())
    }

    fn validate_for_enum(&self) -> syn::Result<()> {
        if self.rename.is_some()
            || self.default.is_some()
            || self.minimum.is_some()
            || self.maximum.is_some()
            || self.minimum_length.is_some()
            || self.maximum_length.is_some()
            || self.validate.is_some()
            || self.unknown_fields.is_some()
            || self.rest
            || self.nested
        {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "enum containers support only `rename_all`",
            ));
        }
        Ok(())
    }

    fn validate_for_field(&self, ident: &syn::Ident) -> syn::Result<()> {
        if self.rename_all.is_some() || self.unknown_fields.is_some() {
            return Err(syn::Error::new_spanned(
                ident,
                "field cannot use a struct-level schema attribute",
            ));
        }

        if self.rest
            && (self.rename.is_some()
                || self.default.is_some()
                || self.minimum.is_some()
                || self.maximum.is_some()
                || self.minimum_length.is_some()
                || self.maximum_length.is_some()
                || self.validate.is_some()
                || self.nested)
        {
            return Err(syn::Error::new_spanned(
                ident,
                "a rest field cannot use other schema attributes",
            ));
        }
        Ok(())
    }

    fn validate_for_variant(&self, ident: &syn::Ident) -> syn::Result<()> {
        if self.rename_all.is_some()
            || self.default.is_some()
            || self.minimum.is_some()
            || self.maximum.is_some()
            || self.minimum_length.is_some()
            || self.maximum_length.is_some()
            || self.validate.is_some()
            || self.unknown_fields.is_some()
            || self.rest
            || self.nested
        {
            return Err(syn::Error::new_spanned(
                ident,
                "enum variants support only `rename`",
            ));
        }
        Ok(())
    }
}

enum DefaultValue {
    Default,
    Expression(Expr),
}

#[derive(Clone, Copy)]
enum RenameAll {
    None,
    Lowercase,
    Uppercase,
    CamelCase,
    PascalCase,
    SnakeCase,
    ScreamingSnakeCase,
    KebabCase,
    ScreamingKebabCase,
}

impl RenameAll {
    fn parse(value: Option<&str>) -> syn::Result<Self> {
        match value {
            None => Ok(Self::None),
            Some("lowercase") => Ok(Self::Lowercase),
            Some("UPPERCASE") => Ok(Self::Uppercase),
            Some("camelCase") => Ok(Self::CamelCase),
            Some("PascalCase") => Ok(Self::PascalCase),
            Some("snake_case") => Ok(Self::SnakeCase),
            Some("SCREAMING_SNAKE_CASE") => Ok(Self::ScreamingSnakeCase),
            Some("kebab-case") => Ok(Self::KebabCase),
            Some("SCREAMING-KEBAB-CASE") => Ok(Self::ScreamingKebabCase),
            Some(value) => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("unsupported rename_all rule `{value}`"),
            )),
        }
    }

    fn apply(self, name: &str) -> String {
        if matches!(self, Self::None) {
            return name.to_owned();
        }

        let words = split_words(name);

        match self {
            Self::None => name.to_owned(),
            Self::Lowercase => words.concat(),
            Self::Uppercase => words.concat().to_ascii_uppercase(),
            Self::CamelCase => {
                let mut words = words.into_iter();
                let first = words.next().unwrap_or_default();
                first + &words.map(capitalize).collect::<String>()
            }
            Self::PascalCase => words.into_iter().map(capitalize).collect(),
            Self::SnakeCase => words.join("_"),
            Self::ScreamingSnakeCase => words.join("_").to_ascii_uppercase(),
            Self::KebabCase => words.join("-"),
            Self::ScreamingKebabCase => words.join("-").to_ascii_uppercase(),
        }
    }
}

fn split_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let characters = name.chars().collect::<Vec<_>>();

    for (index, character) in characters.iter().copied().enumerate() {
        if character == '_' || character == '-' {
            if !current.is_empty() {
                words.push(current.to_ascii_lowercase());
                current.clear();
            }
            continue;
        }

        let previous_is_lower = index > 0 && characters[index - 1].is_ascii_lowercase();
        let next_is_lower = characters
            .get(index + 1)
            .is_some_and(|next| next.is_ascii_lowercase());

        if character.is_ascii_uppercase()
            && !current.is_empty()
            && (previous_is_lower || next_is_lower)
        {
            words.push(current.to_ascii_lowercase());
            current.clear();
        }

        current.push(character);
    }

    if !current.is_empty() {
        words.push(current.to_ascii_lowercase());
    }

    words
}

fn capitalize(word: String) -> String {
    let mut characters = word.chars();
    let Some(first) = characters.next() else {
        return word;
    };

    first.to_ascii_uppercase().to_string() + characters.as_str()
}

fn generic_inner<'type_>(field_type: &'type_ Type, wrapper: &str) -> Option<&'type_ Type> {
    let Type::Path(path) = field_type else {
        return None;
    };
    let segment = path.path.segments.last()?;

    if segment.ident != wrapper {
        return None;
    }

    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let [GenericArgument::Type(inner)] = arguments.args.iter().collect::<Vec<_>>().as_slice()
    else {
        return None;
    };

    Some(inner)
}

fn is_type_named(field_type: &Type, expected: &str) -> bool {
    matches!(
        field_type,
        Type::Path(path)
            if path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == expected)
    )
}
