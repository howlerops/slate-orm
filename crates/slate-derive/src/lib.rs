//! `#[derive(Record)]` for slate-orm.
//!
//! The macro's job is to keep a Rust struct and its table definition from
//! drifting apart. It emits calls to the same public builders a hand-written
//! schema uses, so there is one definition path and one set of validation
//! rules; what the macro adds is that the column list, the field list and the
//! row codec are generated from a single source and cannot disagree.
//!
//! It never inspects field *types*. Column type and nullability come from the
//! `Field` trait's associated constants, so a type alias, or a newtype with its
//! own `Field` impl, works exactly like the type it stands for — whereas a
//! macro that pattern-matched the token `Option` would silently get both wrong.
//!
//! # Attributes
//!
//! On the struct:
//!
//! ```text
//! #[record(table = "users", id = 1)]
//! #[record(version = 3)]                  // schema version stamped on rows
//! #[record(tenant = "tenant_id")]         // enables physical tenant scoping
//! #[record(index(name = "by_a_b", id = 5, unique, columns("a", desc("b"))))]
//! ```
//!
//! On a field:
//!
//! ```text
//! #[record(pk)]                           // part of the primary key, in order
//! #[record(rename = "email_address")]     // column name, if not the field name
//! #[record(added_in = 2)]                 // introduced at this schema version
//! #[record(index(name = "by_email", id = 10, unique, desc))]
//! ```

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned as _;
use syn::{Data, DeriveInput, Fields, Ident, LitInt, LitStr, Token, Type, parenthesized};

/// Derive [`Record`](../slate_orm/trait.Record.html) for a struct with named
/// fields. See the module docs for the attributes.
#[proc_macro_derive(Record, attributes(record))]
pub fn derive_record(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// One column of an index, as written in an attribute.
#[derive(Clone)]
struct IndexColumnSpec {
    name: String,
    descending: bool,
    span: Span,
}

impl Parse for IndexColumnSpec {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(LitStr) {
            let literal: LitStr = input.parse()?;
            return Ok(Self {
                name: literal.value(),
                descending: false,
                span: literal.span(),
            });
        }
        let direction: Ident = input.parse()?;
        let inner;
        parenthesized!(inner in input);
        let literal: LitStr = inner.parse()?;
        let descending = match direction.to_string().as_str() {
            "desc" => true,
            "asc" => false,
            _ => {
                return Err(syn::Error::new(
                    direction.span(),
                    "expected a column name, `asc(\"column\")`, or `desc(\"column\")`",
                ));
            }
        };
        Ok(Self {
            name: literal.value(),
            descending,
            span: literal.span(),
        })
    }
}

/// An index as written in an attribute.
#[derive(Clone)]
struct IndexSpec {
    name: String,
    id: u32,
    unique: bool,
    columns: Vec<IndexColumnSpec>,
    span: Span,
}

/// Parse `index(...)`. `owner` is the field a bare field-level index applies to.
fn parse_index(
    meta: &syn::meta::ParseNestedMeta<'_>,
    owner: Option<&str>,
) -> syn::Result<IndexSpec> {
    let span = meta.path.span();
    let mut name: Option<String> = None;
    let mut id: Option<u32> = None;
    let mut unique = false;
    let mut descending = false;
    let mut columns: Vec<IndexColumnSpec> = Vec::new();

    meta.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if meta.path.is_ident("unique") {
            unique = true;
        } else if meta.path.is_ident("desc") {
            descending = true;
        } else if meta.path.is_ident("columns") {
            let content;
            parenthesized!(content in meta.input);
            columns = Punctuated::<IndexColumnSpec, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();
        } else {
            return Err(meta.error(
                "unknown index option; expected `name`, `id`, `unique`, `desc` or `columns`",
            ));
        }
        Ok(())
    })?;

    let Some(name) = name else {
        return Err(syn::Error::new(span, "index needs `name = \"...\"`"));
    };
    let Some(id) = id else {
        return Err(syn::Error::new(
            span,
            "index needs `id = N`; ids are part of the on-disk layout, so they are assigned \
             explicitly rather than derived from the name",
        ));
    };

    if columns.is_empty() {
        match owner {
            Some(field) => columns.push(IndexColumnSpec {
                name: field.to_owned(),
                descending,
                span,
            }),
            None => {
                return Err(syn::Error::new(
                    span,
                    "an index on the struct needs `columns(...)`; only an index written on a \
                     field may leave them out",
                ));
            }
        }
    } else if descending {
        return Err(syn::Error::new(
            span,
            "`desc` applies to a field-level index; inside `columns(...)` write `desc(\"column\")`",
        ));
    }

    Ok(IndexSpec {
        name,
        id,
        unique,
        columns,
        span,
    })
}

/// Everything the macro learned about one field.
struct FieldSpec {
    ident: Ident,
    ty: Type,
    column: String,
    primary_key: bool,
    added_in: Option<u32>,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "`Record` can only be derived for a struct",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "`Record` needs named fields; a column has to have a name",
        ));
    };

    // --- struct-level attributes
    let mut table_name: Option<String> = None;
    let mut table_id: Option<u32> = None;
    let mut version: u32 = 0;
    let mut tenant: Option<(String, Span)> = None;
    let mut indexes: Vec<IndexSpec> = Vec::new();

    for attr in &input.attrs {
        if !attr.path().is_ident("record") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table") {
                table_name = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("id") {
                table_id = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
            } else if meta.path.is_ident("version") {
                version = meta.value()?.parse::<LitInt>()?.base10_parse()?;
            } else if meta.path.is_ident("tenant") {
                let literal = meta.value()?.parse::<LitStr>()?;
                tenant = Some((literal.value(), literal.span()));
            } else if meta.path.is_ident("index") {
                indexes.push(parse_index(&meta, None)?);
            } else {
                return Err(meta.error(
                    "unknown option; expected `table`, `id`, `version`, `tenant` or `index`",
                ));
            }
            Ok(())
        })?;
    }

    let Some(table_name) = table_name else {
        return Err(syn::Error::new_spanned(
            input,
            "missing `#[record(table = \"...\", id = N)]`",
        ));
    };
    let Some(table_id) = table_id else {
        return Err(syn::Error::new_spanned(
            input,
            "missing `id = N` in `#[record(...)]`; table ids are part of the on-disk layout, so \
             they are assigned explicitly",
        ));
    };

    // --- field-level attributes
    let mut fields: Vec<FieldSpec> = Vec::new();
    let mut primary_key: Vec<String> = Vec::new();

    for field in &named.named {
        let Some(ident) = field.ident.clone() else {
            continue;
        };
        let mut column = ident.to_string();
        let mut is_pk = false;
        let mut added_in: Option<u32> = None;
        let mut renamed: Option<String> = None;

        // Names are resolved after the loop, so index specs on this field are
        // collected against the field's *final* column name.
        let mut field_indexes: Vec<IndexSpec> = Vec::new();

        for attr in &field.attrs {
            if !attr.path().is_ident("record") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("pk") {
                    is_pk = true;
                } else if meta.path.is_ident("rename") {
                    renamed = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("added_in") {
                    added_in = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
                } else if meta.path.is_ident("index") {
                    field_indexes.push(parse_index(&meta, Some("\0self"))?);
                } else {
                    return Err(meta
                        .error("unknown option; expected `pk`, `rename`, `added_in` or `index`"));
                }
                Ok(())
            })?;
        }

        if let Some(name) = renamed {
            column = name;
        }
        for mut spec in field_indexes {
            for c in &mut spec.columns {
                if c.name == "\0self" {
                    c.name = column.clone();
                }
            }
            indexes.push(spec);
        }
        if is_pk {
            primary_key.push(column.clone());
        }

        fields.push(FieldSpec {
            ident,
            ty: field.ty.clone(),
            column,
            primary_key: is_pk,
            added_in,
        });
    }

    validate(input, &fields, &primary_key, tenant.as_ref(), &indexes)?;

    // --- code generation
    let column_stmts = fields.iter().map(|f| {
        let name = &f.column;
        let ty = &f.ty;
        match f.added_in {
            Some(v) => quote! {
                builder = builder.added_column(
                    #name,
                    <#ty as ::slate_orm::Field>::VALUE_TYPE,
                    #v,
                );
            },
            None => quote! {
                // Nullability comes from the type, not from the macro reading it.
                builder = if <#ty as ::slate_orm::Field>::NULLABLE {
                    builder.nullable_column(#name, <#ty as ::slate_orm::Field>::VALUE_TYPE)
                } else {
                    builder.column(#name, <#ty as ::slate_orm::Field>::VALUE_TYPE)
                };
            },
        }
    });

    let pk_names = primary_key.iter();
    let tenant_stmt = tenant.as_ref().map(|(name, _)| {
        quote! { builder = builder.tenant_column(#name); }
    });

    let index_stmts = indexes.iter().map(|spec| {
        let name = &spec.name;
        let id = spec.id;
        let columns = spec.columns.iter().map(|c| {
            let column = &c.name;
            if c.descending {
                quote! { index = index.column_with(#column, ::slate_orm::Direction::Desc); }
            } else {
                quote! { index = index.column(#column); }
            }
        });
        let unique = spec.unique.then(|| quote! { index = index.unique(); });
        quote! {
            builder = builder.index({
                let mut index = ::slate_orm::IndexDef::builder(
                    #name,
                    ::slate_orm::IndexId(#id),
                );
                #(#columns)*
                #unique
                index
            });
        }
    });

    let idents: Vec<&Ident> = fields.iter().map(|f| &f.ident).collect();
    let types: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();
    let columns: Vec<&String> = fields.iter().map(|f| &f.column).collect();
    let positions: Vec<usize> = (0..fields.len()).collect();
    let field_count = fields.len();

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        // The build below can only fail on a schema mistake in this derive's
        // own attributes, which is a programming error rather than a runtime
        // condition, so it reports itself loudly instead of widening every
        // caller's signature.
        #[allow(clippy::panic, clippy::expect_used)]
        impl #impl_generics ::slate_orm::Record for #ident #ty_generics #where_clause {
            fn table() -> &'static ::slate_orm::TableDef {
                static TABLE: ::std::sync::OnceLock<::slate_orm::TableDef> =
                    ::std::sync::OnceLock::new();
                TABLE.get_or_init(|| {
                    let mut builder = ::slate_orm::TableDef::builder(
                        #table_name,
                        ::slate_orm::TableId(#table_id),
                    );
                    #(#column_stmts)*
                    builder = builder.primary_key([#(#pk_names),*]);
                    #tenant_stmt
                    #(#index_stmts)*
                    builder = builder.schema_version(#version);
                    match builder.build() {
                        ::core::result::Result::Ok(table) => table,
                        ::core::result::Result::Err(error) => ::core::panic!(
                            "invalid schema derived for `{}`: {}",
                            ::core::stringify!(#ident),
                            error,
                        ),
                    }
                })
            }

            fn to_row(&self) -> ::slate_orm::Row {
                ::slate_orm::Row::new(::std::vec![
                    #(<#types as ::slate_orm::Field>::to_value(&self.#idents)),*
                ])
            }

            fn from_row(
                row: &::slate_orm::Row,
            ) -> ::core::result::Result<Self, ::slate_orm::RecordError> {
                let values = row.values();
                if values.len() != #field_count {
                    return ::core::result::Result::Err(
                        ::slate_orm::RecordError::ColumnCount {
                            table: #table_name,
                            expected: #field_count,
                            actual: values.len(),
                        },
                    );
                }
                ::core::result::Result::Ok(Self {
                    #(
                        #idents: <#types as ::slate_orm::Field>::from_value(
                            values.get(#positions).unwrap_or(&::slate_orm::Value::Null),
                        )
                        .map_err(|source| ::slate_orm::RecordError::Field {
                            table: #table_name,
                            column: #columns,
                            source,
                        })?,
                    )*
                })
            }
        }
    })
}

/// Reject at compile time what can be known at compile time.
///
/// The builder re-checks all of this at runtime, but a mistake in a derive
/// attribute is a mistake in source code, and belongs in the compiler's output
/// with a span attached rather than in a panic on first use.
fn validate(
    input: &DeriveInput,
    fields: &[FieldSpec],
    primary_key: &[String],
    tenant: Option<&(String, Span)>,
    indexes: &[IndexSpec],
) -> syn::Result<()> {
    if primary_key.is_empty() {
        return Err(syn::Error::new_spanned(
            input,
            "no primary key: mark at least one field with `#[record(pk)]`",
        ));
    }

    let known = |name: &str| fields.iter().any(|f| f.column == name);

    if let Some((tenant_name, span)) = tenant {
        if !known(tenant_name) {
            return Err(syn::Error::new(
                *span,
                format!("`{tenant_name}` is not a column of this struct"),
            ));
        }
        // The kernel turns a tenant restriction into a key prefix, which only
        // works if the tenant leads the key.
        if primary_key.first().map(String::as_str) != Some(tenant_name.as_str()) {
            return Err(syn::Error::new(
                *span,
                format!(
                    "the tenant column must be the first `#[record(pk)]` field, so that tenant \
                     scoping is a key prefix rather than a filter; move `{tenant_name}` to the \
                     front of the primary key"
                ),
            ));
        }
    }

    for (i, spec) in indexes.iter().enumerate() {
        if spec.columns.is_empty() {
            return Err(syn::Error::new(spec.span, "index has no columns"));
        }
        for earlier in indexes.iter().take(i) {
            if earlier.name == spec.name {
                return Err(syn::Error::new(
                    spec.span,
                    format!("two indexes are named `{}`", spec.name),
                ));
            }
            if earlier.id == spec.id {
                return Err(syn::Error::new(
                    spec.span,
                    format!(
                        "indexes `{}` and `{}` share id {}; ids are part of the on-disk layout \
                         and must be distinct",
                        earlier.name, spec.name, spec.id
                    ),
                ));
            }
        }
        for column in &spec.columns {
            if !known(&column.name) {
                return Err(syn::Error::new(
                    column.span,
                    format!("`{}` is not a column of this struct", column.name),
                ));
            }
        }
    }

    for (i, field) in fields.iter().enumerate() {
        if fields.iter().take(i).any(|f| f.column == field.column) {
            return Err(syn::Error::new(
                field.ident.span(),
                format!("two fields map to the column `{}`", field.column),
            ));
        }
        if field.primary_key && field.added_in.is_some() {
            return Err(syn::Error::new(
                field.ident.span(),
                "a primary key column cannot be added in a later schema version: older rows \
                 have no value for it, and a key component may not be null",
            ));
        }
    }

    Ok(())
}
