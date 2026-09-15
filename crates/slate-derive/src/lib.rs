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
//! #[record(index(name = "live", id = 6, columns("a"),
//!                only_where(Expr::is_null(deleted_at))))]
//! #[record(has_many(Book, foreign = author_id))]       // one-to-many
//! #[record(belongs_to(Author, local = author_id))]     // the other direction
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
//!
//! # Partial indexes
//!
//! `only_where(...)` is the one attribute holding a Rust *expression* rather
//! than a literal, and it exists because the two things a partial index's
//! predicate needs are exactly the two things this macro is placed to give.
//!
//! The first is ordinals. `IndexBuilder::only_where` names columns by
//! `Ordinal`, and a helper that resolved a name by building the table would
//! recurse — building the table is what evaluates the predicate. So the
//! expression is emitted with every **field** ident bound to its ordinal, and
//! the macro knows those positions without building anything. Fields, not
//! columns: `#[record(rename = "...")]` changes the stored name and not the
//! name in the struct, and the struct is what the reader of this attribute is
//! looking at. It is the same choice `COLUMNS` already makes.
//!
//! Binding the field names shadows anything of the same name in scope for the
//! length of the expression. That is a real cost and it was accepted, because
//! an `Ordinal` is neither callable nor arithmetic: the shadowing can turn a
//! predicate into a type error, but not into a different predicate.
//!
//! The second is the type. `only_where` accepts any `Predicate`, and the
//! planner only reads a predicate it can downcast to `slate_kernel::Expr` — so
//! a predicate of some other type is not a broken index but a *silently
//! unused* one, which is the failure this macro should not be able to emit.
//! The expression is therefore pinned to `Expr` on the way in, making the
//! mistake a mismatched-types error at the attribute rather than a plan that
//! quietly never picks the index.
//!
//! # Relationships
//!
//! `has_many` and `belongs_to` emit a `Related` impl, which `load_related`
//! turns into one read for a whole set of parents. They name columns by
//! **field ident** for the same reason `only_where` does: a string would be
//! resolved at runtime against a table this macro cannot see, so a typo becomes
//! a panic on first use — or, if it happens to name a real column, a
//! relationship over the wrong one. The local side is checked here, against
//! this struct's fields, with a span on the attribute; the foreign side is
//! emitted as `Other::COLUMNS.field`, which the compiler resolves.
//!
//! Only one of the two sides has a default, and only in one direction. A
//! `belongs_to`'s foreign column defaults to the other side's primary key,
//! because that is what a foreign key points at. Nothing else does: a child's
//! primary key is not its foreign key, and a struct may belong to two things,
//! so a default for either of those would compile and be wrong. The one
//! default that cannot be resolved here — the other side's key, on a table this
//! macro has not got — is looked up at first use and panics if that key is
//! composite, rather than take the first column and relate on a key prefix.

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
    /// The `only_where(...)` predicate, emitted verbatim. Held unparsed beyond
    /// `syn::Expr` on purpose: the macro has no business knowing which
    /// predicates `Expr` can express, and one that learned would have to be
    /// taught again every time the kernel gains a form.
    predicate: Option<syn::Expr>,
    span: Span,
}

/// A relationship as written in an attribute.
#[derive(Clone)]
struct RelationSpec {
    /// The type on the other end, emitted verbatim. A `Type` rather than an
    /// `Ident` so a relationship can name `crate::catalog::Book` without the
    /// struct having to import it.
    other: Type,
    /// True for `has_many`, false for `belongs_to`. The two differ only in
    /// which side may be defaulted, which is why one spec covers both.
    has_many: bool,
    local: Option<Ident>,
    foreign: Option<Ident>,
    span: Span,
}

/// Parse `has_many(Other, foreign = field)` or `belongs_to(Other, local = field)`.
///
/// The columns are named by **field ident**, not by a string, and that is the
/// whole design. A string would be checked at runtime against a table built
/// from the other type, which turns a typo into a panic on first use — or worse
/// into a relationship over the wrong column if the typo happens to name one.
/// An ident is checked by the compiler: the local side against this struct's
/// fields here in the macro, with a span on the attribute, and the foreign side
/// by emitting `Other::COLUMNS.field`, which does not compile unless `Other` is
/// a `Record` with a field of that name. It is the same trade `only_where`
/// makes, for the same reason.
fn parse_relation(
    meta: &syn::meta::ParseNestedMeta<'_>,
    has_many: bool,
) -> syn::Result<RelationSpec> {
    let span = meta.path.span();
    let content;
    parenthesized!(content in meta.input);
    let other: Type = content.parse()?;

    let mut local = None;
    let mut foreign = None;
    while content.peek(Token![,]) {
        content.parse::<Token![,]>()?;
        if content.is_empty() {
            break;
        }
        let key: Ident = content.parse()?;
        content.parse::<Token![=]>()?;
        let value: Ident = content.parse()?;
        match key.to_string().as_str() {
            "local" => local = Some(value),
            "foreign" => foreign = Some(value),
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!("unknown option `{other}`; expected `local` or `foreign`"),
                ));
            }
        }
    }
    Ok(RelationSpec {
        other,
        has_many,
        local,
        foreign,
        span,
    })
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
    let mut predicate: Option<syn::Expr> = None;

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
        } else if meta.path.is_ident("only_where") {
            // Two of them would mean one predicate maintained and one ignored,
            // and the ignored one would read like it were in force.
            if predicate.is_some() {
                return Err(meta.error("`only_where` given twice; an index has one predicate"));
            }
            let content;
            parenthesized!(content in meta.input);
            predicate = Some(content.parse()?);
            if !content.is_empty() {
                return Err(content.error(
                    "`only_where` takes a single predicate expression; combine terms with \
                     `Expr::and` rather than a comma",
                ));
            }
        } else {
            return Err(meta.error(
                "unknown index option; expected `name`, `id`, `unique`, `desc`, `columns` or \
                 `only_where`",
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
        predicate,
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
    let mut relations: Vec<RelationSpec> = Vec::new();

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
            } else if meta.path.is_ident("has_many") {
                relations.push(parse_relation(&meta, true)?);
            } else if meta.path.is_ident("belongs_to") {
                relations.push(parse_relation(&meta, false)?);
            } else {
                return Err(meta.error(
                    "unknown option; expected `table`, `id`, `version`, `tenant`, `index`, \
                     `has_many` or `belongs_to`",
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
    validate_relations(&fields, &relations)?;

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

    // Every field ident bound to its ordinal, for a partial index's predicate
    // to name columns by the name they have in the struct. Emitted per
    // predicate rather than once around the whole table build, so a struct with
    // no partial index carries none of it — and so the shadowing these
    // bindings do reaches no further than the expression that asked for it.
    let ordinal_bindings = {
        let names = fields.iter().map(|f| &f.ident);
        let ordinals = (0..fields.len()).map(|i| quote! { ::slate_orm::Ordinal(#i) });
        quote! {
            // A predicate names the columns it needs; the rest are here to be
            // nameable, not to be used. Non-snake-case field names have already
            // been reported on the struct itself.
            #[allow(unused_variables, non_snake_case)]
            let (#(#names,)*) = (#(#ordinals,)*);
        }
    };

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
        let predicate = spec.predicate.as_ref().map(|expr| {
            // The annotation is the point, not the binding: `only_where` takes
            // any `Predicate`, and one that is not an `Expr` is an index the
            // planner can never read. Pinned here so that is a type error on
            // the attribute instead of an index that silently goes unused.
            quote! {
                index = index.only_where({
                    #ordinal_bindings
                    let predicate: ::slate_orm::Expr = #expr;
                    predicate
                });
            }
        });
        quote! {
            builder = builder.index({
                let mut index = ::slate_orm::IndexDef::builder(
                    #name,
                    ::slate_orm::IndexId(#id),
                );
                #(#columns)*
                #unique
                #predicate
                index
            });
        }
    });

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // One `Related` impl per declared relationship. Emitted outside the
    // `Record` impl because they are separate trait impls on the same type, and
    // because a relationship to a type that is not a `Record` should fail on
    // the relationship rather than take the whole table definition with it.
    let relation_impls: Vec<TokenStream2> = relations
        .iter()
        .map(|spec| {
            let other = &spec.other;
            let self_ident = &input.ident;

            // The local side is a column of *this* struct, whose ordinals the
            // macro already knows, so it is emitted as a constant and the
            // checking happened in `validate_relations` with a span on the
            // attribute.
            let local = match &spec.local {
                Some(name) => {
                    let at = fields
                        .iter()
                        .position(|f| f.ident == *name)
                        .unwrap_or_default();
                    quote! { ::slate_orm::Ordinal(#at) }
                }
                None => {
                    let at = fields
                        .iter()
                        .position(|f| f.primary_key)
                        .unwrap_or_default();
                    quote! { ::slate_orm::Ordinal(#at) }
                }
            };

            // The foreign side belongs to a type this macro cannot see, so it
            // is emitted as a reference the *compiler* resolves:
            // `Other::COLUMNS.field` does not exist unless `Other` derives
            // `Record` and has that field.
            let foreign = match &spec.foreign {
                Some(name) => quote! { <#other>::COLUMNS.#name },
                None => quote! {
                    // Only reachable for `belongs_to` with no `foreign`, which
                    // means "the other side's primary key". That is a lookup on
                    // a table this macro has not got, so it happens once at
                    // first use. It panics rather than guessing, on the same
                    // grounds as the schema build above: it is a mistake in
                    // source code, it is the same every run, and a wrong answer
                    // here is a relationship over the wrong column.
                    match <#other as ::slate_orm::Record>::table().primary_key() {
                        [only] => *only,
                        key => ::core::panic!(
                            "`{}` cannot default its foreign column: `{}` has a primary key of \
                             {} columns, so there is no single one to match on. Name it: \
                             `#[record(belongs_to({}, local = ..., foreign = <field>))]`",
                            ::core::stringify!(#self_ident),
                            ::core::stringify!(#other),
                            key.len(),
                            ::core::stringify!(#other),
                        ),
                    }
                },
            };

            quote! {
                #[allow(clippy::panic)]
                impl #impl_generics ::slate_orm::Related<#other>
                    for #self_ident #ty_generics #where_clause
                {
                    fn local() -> ::slate_orm::Ordinal {
                        #local
                    }
                    fn foreign() -> ::slate_orm::Ordinal {
                        #foreign
                    }
                }
            }
        })
        .collect();

    let idents: Vec<&Ident> = fields.iter().map(|f| &f.ident).collect();
    let types: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();
    let columns: Vec<&String> = fields.iter().map(|f| &f.column).collect();
    let positions: Vec<usize> = (0..fields.len()).collect();
    let field_count = fields.len();

    let ident = &input.ident;

    // Ordinals are known here, so column references can be constants instead of
    // a fallible name lookup at every call site. Writing a filter is the most
    // common thing a caller does, and `table().ordinal_of("x").unwrap()` is a
    // panic waiting in otherwise ordinary code.
    let columns_ident = syn::Ident::new(&format!("{ident}Columns"), ident.span());
    let visibility = &input.vis;
    let column_docs = fields.iter().map(|f| {
        let name = &f.column;
        format!("Ordinal of the `{name}` column.")
    });
    let column_fields = idents.iter().zip(positions.iter()).map(|(name, index)| {
        quote! { #name: ::slate_orm::Ordinal(#index) }
    });
    let column_decls = idents.iter().zip(column_docs).map(|(name, doc)| {
        quote! {
            #[doc = #doc]
            pub #name: ::slate_orm::Ordinal
        }
    });
    let columns_doc = format!("Column ordinals of [`{ident}`], for building predicates.");

    Ok(quote! {
        #(#relation_impls)*

        #[doc = #columns_doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #visibility struct #columns_ident {
            #(#column_decls,)*
        }

        impl #impl_generics #ident #ty_generics #where_clause {
            #[doc = #columns_doc]
            ///
            /// Ordinals are fixed by the field order, so these are constants
            /// rather than a name lookup that could fail.
            pub const COLUMNS: #columns_ident = #columns_ident {
                #(#column_fields,)*
            };
        }

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

/// Check the half of a relationship this macro can see.
///
/// The other half is checked by the compiler, because `Other::COLUMNS.field` is
/// emitted verbatim: a foreign column that is not a field of a `Record` is a
/// resolution error at the call site, which needs nothing from here.
///
/// What is here is the local side, and the two defaults that can fail to exist.
fn validate_relations(fields: &[FieldSpec], relations: &[RelationSpec]) -> syn::Result<()> {
    for spec in relations {
        // A has-many's foreign column is the child's foreign key, and a child's
        // foreign key has no relationship to its primary key — there is nothing
        // sensible to default it to, so it is required rather than guessed.
        if spec.has_many && spec.foreign.is_none() {
            return Err(syn::Error::new(
                spec.span,
                "`has_many` needs `foreign = <field>`: the column of the other type that \
                 holds this one's key. There is no default, because the child's own primary \
                 key is not it",
            ));
        }
        // A belongs-to's local column is this struct's foreign key, same
        // argument. Its *foreign* column does default, to the other side's
        // primary key, which is what a foreign key points at.
        if !spec.has_many && spec.local.is_none() {
            return Err(syn::Error::new(
                spec.span,
                "`belongs_to` needs `local = <field>`: the field of this struct holding the \
                 other one's key. It is not defaulted, because a struct may belong to two \
                 things and the primary key is neither",
            ));
        }

        if let Some(name) = &spec.local {
            if !fields.iter().any(|f| f.ident == *name) {
                return Err(syn::Error::new(
                    name.span(),
                    format!("`{name}` is not a field of this struct"),
                ));
            }
        } else {
            // Only a has-many gets here, and its local column defaults to this
            // struct's primary key. A composite one has no single ordinal to
            // return, and picking the first would be a relationship over a key
            // prefix that matches rows from every other tenant.
            let keys = fields.iter().filter(|f| f.primary_key).count();
            if keys != 1 {
                return Err(syn::Error::new(
                    spec.span,
                    format!(
                        "this struct has a primary key of {keys} columns, so `has_many` \
                         cannot default its local column; name the one that matches with \
                         `local = <field>`"
                    ),
                ));
            }
        }
    }
    Ok(())
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
