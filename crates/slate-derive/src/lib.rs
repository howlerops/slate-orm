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
//! #[record(check(name = "priced",
//!                predicate(Expr::compare(price, CmpOp::Ge, Value::I64(0))),
//!                column = "price", message = "Price cannot be negative."))]
//! #[record(foreign_key(name = "books_author", parent = Author,
//!                      column = "author_id", on_delete = cascade))]
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
//! #[record(check(name = "has_an_at", predicate(Expr::matches(email, "@")),
//!                message = "That is not an email address."))]
//! #[record(foreign_key(name = "books_author", parent = Author))]
//! ```
//!
//! # Foreign keys
//!
//! `foreign_key(...)` names the parent as a **type**, so a typo is a compile
//! error rather than a `NOT_FOUND` from the catalog at startup, and the id
//! comes from `slate_orm::Record::table_id` rather than from `table()` —
//! because `table()` caches behind a `OnceLock` and reading it here would
//! re-enter that lock the moment a table referenced itself.
//!
//! Written on a field, the referencing column is that field, after `rename`.
//! Written on the struct, every column is named and **order is
//! load-bearing**: they are matched against the parent's primary key in the
//! order written, so a key into a tenant-scoped parent names the tenant
//! column first. `on_delete` is `restrict` unless it says `cascade`.
//!
//! # Checks
//!
//! `check(...)` takes the same `predicate(...)` expression `only_where` does,
//! for the same two reasons, and adds the `column` and `message` a
//! `CheckDef` carries. A check written **on a field** defaults `column` to
//! that field — resolved after any `rename`, so it is the name the wire
//! carries rather than the field ident — which is the point of writing it
//! there. An explicit `column` still wins, so a check about two columns can
//! sit beside one of them and name it.
//!
//! A `column` naming nothing is a **compile error**. The TOML loader refuses
//! the same mistake at startup; here it can be refused before the binary
//! exists, which matters because the symptom otherwise is a form rendering an
//! error beside no field at all, at the first refused write.
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

/// A `CHECK` as written in an attribute.
#[derive(Clone)]
struct CheckSpec {
    name: String,
    /// The `predicate(...)` expression, emitted verbatim, for the same reason
    /// [`IndexSpec::predicate`] is: the macro has no business knowing which
    /// predicates `Expr` can express.
    predicate: syn::Expr,
    /// `None` when the check names no column, which is the honest answer for
    /// `discount <= price`. A field-level `check(...)` fills this in with the
    /// field it sits on; `"\0self"` is the placeholder until the field's
    /// final column name is known, exactly as `IndexColumnSpec` does it.
    column: Option<String>,
    message: Option<String>,
    span: Span,
}

/// Parse `check(name = "...", predicate(expr), column = "...", message = "...")`.
///
/// `owner` is `Some` for a check written on a field, and then `column`
/// defaults to that field. That default is the whole reason the field position
/// is worth having: a check written beside the column it is about should not
/// have to repeat the column's name, and the failure mode of repeating it —
/// renaming the field and not the string — is the one
/// `ledger/2026-09-19-a-check-that-names-its-field.md` argued the `column`
/// field exists to prevent.
fn parse_check(
    meta: &syn::meta::ParseNestedMeta<'_>,
    owner: Option<&str>,
) -> syn::Result<CheckSpec> {
    let span = meta.path.span();
    let mut name: Option<String> = None;
    let mut predicate: Option<syn::Expr> = None;
    let mut column: Option<String> = None;
    let mut message: Option<String> = None;

    meta.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("predicate") {
            // Two of them would mean one rule enforced and one ignored, and
            // the ignored one would read like it were in force — the same
            // refusal `only_where` makes, for the same reason.
            if predicate.is_some() {
                return Err(meta.error("`predicate` given twice; a check has one"));
            }
            let content;
            parenthesized!(content in meta.input);
            predicate = Some(content.parse()?);
            if !content.is_empty() {
                return Err(content.error(
                    "`predicate` takes a single expression; combine terms with `Expr::and` \
                     rather than a comma",
                ));
            }
        } else if meta.path.is_ident("column") {
            column = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("message") {
            message = Some(meta.value()?.parse::<LitStr>()?.value());
        } else {
            return Err(meta.error(
                "unknown check option; expected `name`, `predicate`, `column` or `message`",
            ));
        }
        Ok(())
    })?;

    let Some(name) = name else {
        return Err(syn::Error::new(span, "check needs `name = \"...\"`"));
    };
    let Some(predicate) = predicate else {
        return Err(syn::Error::new(
            span,
            "check needs `predicate(...)`; a check with no predicate is a name that refuses \
             nothing",
        ));
    };

    Ok(CheckSpec {
        name,
        predicate,
        // An explicit `column` on a field-level check wins over the field it
        // sits on. Writing one is how `discount <= price` says it is about
        // `discount` while living beside `price`, and refusing it would make
        // the field position strictly less expressive than the struct one.
        column: column.or_else(|| owner.map(str::to_owned)),
        message,
        span,
    })
}

/// A `FOREIGN KEY` as written in an attribute.
#[derive(Clone)]
struct ForeignKeySpec {
    name: String,
    /// The parent type, emitted verbatim. A `Type` rather than a table name
    /// for the reason `has_many` takes one: a name is resolved at runtime and
    /// a typo in it is a `NOT_FOUND` from the catalog, where a type is resolved
    /// by the compiler against the parent's own `#[record(id = N)]`.
    parent: syn::Type,
    /// The referencing columns, in the parent's primary key order. A
    /// field-level `foreign_key(...)` leaves the `"\0self"` placeholder here
    /// until the field's final column name is known, exactly as
    /// [`CheckSpec::column`] does.
    columns: Vec<String>,
    /// `None` means the builder's default, which is `Restrict`. Kept optional
    /// rather than defaulted here so the emission can stay silent when nothing
    /// was written, and the default lives in one place.
    on_delete: Option<Ident>,
    span: Span,
}

/// Parse `foreign_key(name = "...", parent = Type, column = "...", on_delete = cascade)`.
///
/// `owner` is `Some` for a key written on a field, and then that field is the
/// single referencing column. That default is why the field position is worth
/// having: the overwhelmingly common foreign key is one column into a
/// single-column primary key, and making it name itself is the repetition the
/// `rename` failure mode feeds on.
///
/// `column` may be given more than once, and order is load-bearing — the
/// columns are matched against the parent's primary key in the order written,
/// so a key into a tenant-scoped parent names the tenant column first. That is
/// `ForeignKeyBuilder::column`'s contract and this does not reinterpret it.
fn parse_foreign_key(
    meta: &syn::meta::ParseNestedMeta<'_>,
    owner: Option<&str>,
) -> syn::Result<ForeignKeySpec> {
    let span = meta.path.span();
    let mut name: Option<String> = None;
    let mut parent: Option<syn::Type> = None;
    let mut columns: Vec<String> = Vec::new();
    let mut on_delete: Option<Ident> = None;

    meta.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("parent") {
            if parent.is_some() {
                return Err(meta.error(
                    "`parent` given twice; a foreign key references one table, and two would \
                     mean one reference enforced and one ignored",
                ));
            }
            parent = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("column") {
            columns.push(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("on_delete") {
            let word: Ident = meta.value()?.parse()?;
            // Matched here rather than passed through, so a misspelling is an
            // error on the attribute naming both actions rather than an
            // `unresolved variant` pointing into the expansion.
            let action = match word.to_string().as_str() {
                "restrict" => Ident::new("Restrict", word.span()),
                "cascade" => Ident::new("Cascade", word.span()),
                other => {
                    return Err(syn::Error::new(
                        word.span(),
                        format!(
                            "unknown `on_delete` action `{other}`; the two are `restrict` \
                             (the default) and `cascade`"
                        ),
                    ));
                }
            };
            on_delete = Some(action);
        } else {
            return Err(meta.error(
                "unknown foreign key option; expected `name`, `parent`, `column` or `on_delete`",
            ));
        }
        Ok(())
    })?;

    let Some(name) = name else {
        return Err(syn::Error::new(span, "foreign key needs `name = \"...\""));
    };
    let Some(parent) = parent else {
        return Err(syn::Error::new(
            span,
            "foreign key needs `parent = <type>`; the type is what resolves the table id, and \
             there is no default for which table a key references",
        ));
    };
    // An explicit `column` on a field-level key wins over the field it sits on,
    // for the reason an explicit `column` on a field-level check does: a
    // composite key into a tenant-scoped parent has to name the tenant column
    // first, and it should not have to move to a different position to say so.
    if columns.is_empty() {
        match owner {
            Some(field) => columns.push(field.to_owned()),
            None => {
                return Err(syn::Error::new(
                    span,
                    "foreign key needs at least one `column = \"...\"`; written on a field it \
                     defaults to that field, but on the struct there is nothing to default to",
                ));
            }
        }
    }

    Ok(ForeignKeySpec {
        name,
        parent,
        columns,
        on_delete,
        span,
    })
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
    /// The join table, for `has_many(Far, through = Join)`.
    ///
    /// Present only for a many-to-many, and it emits a `Through` impl rather
    /// than a `Related` one: the relationship itself is already expressible as
    /// two `Related`s and needs no declaration. What cannot be inferred is
    /// *which* table is the join table, because a blanket impl over the two
    /// halves leaves the join type unconstrained (`E0207`). So this is a name
    /// for a fact the compiler cannot work out, not a new capability.
    through: Option<Type>,
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
    let mut through = None;
    while content.peek(Token![,]) {
        content.parse::<Token![,]>()?;
        if content.is_empty() {
            break;
        }
        let key: Ident = content.parse()?;
        content.parse::<Token![=]>()?;
        // `through` names a *type*; `local` and `foreign` name fields. Parsed
        // apart because a `Type` swallows an `Ident` and the error for a typo
        // would then be about a missing field rather than a missing option.
        if key == "through" {
            if !has_many {
                return Err(syn::Error::new(
                    key.span(),
                    "`through` belongs on `has_many`; a `belongs_to` through a join table \
                     is the join table's own `has_many`",
                ));
            }
            through = Some(content.parse::<Type>()?);
            continue;
        }
        let value: Ident = content.parse()?;
        match key.to_string().as_str() {
            "local" => local = Some(value),
            "foreign" => foreign = Some(value),
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!("unknown option `{other}`; expected `local`, `foreign` or `through`"),
                ));
            }
        }
    }
    Ok(RelationSpec {
        other,
        has_many,
        local,
        foreign,
        through,
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
    /// `#[record(scale = n)]`, for a decimal column.
    scale: Option<u8>,
    /// `#[record(created_at)]` or `#[record(updated_at)]`.
    ///
    /// A `&'static str` rather than the kernel's `Managed`, because this crate
    /// is a proc macro and depends on nothing it generates code against. The
    /// two spellings are checked here and turned into a path below.
    managed: Option<&'static str>,
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
    let mut checks: Vec<CheckSpec> = Vec::new();
    let mut foreign_keys: Vec<ForeignKeySpec> = Vec::new();
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
            } else if meta.path.is_ident("check") {
                checks.push(parse_check(&meta, None)?);
            } else if meta.path.is_ident("foreign_key") {
                foreign_keys.push(parse_foreign_key(&meta, None)?);
            } else if meta.path.is_ident("has_many") {
                relations.push(parse_relation(&meta, true)?);
            } else if meta.path.is_ident("belongs_to") {
                relations.push(parse_relation(&meta, false)?);
            } else {
                return Err(meta.error(
                    "unknown option; expected `table`, `id`, `version`, `tenant`, `index`, \
                     `check`, `foreign_key`, `has_many` or `belongs_to`",
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
    let mut soft_delete: Option<String> = None;

    for field in &named.named {
        let Some(ident) = field.ident.clone() else {
            continue;
        };
        let mut column = ident.to_string();
        let mut is_pk = false;
        let mut added_in: Option<u32> = None;
        let mut renamed: Option<String> = None;
        let mut scale: Option<u8> = None;
        let mut managed: Option<&'static str> = None;
        let mut is_soft_delete = false;

        // Names are resolved after the loop, so index specs on this field are
        // collected against the field's *final* column name.
        let mut field_indexes: Vec<IndexSpec> = Vec::new();
        let mut field_checks: Vec<CheckSpec> = Vec::new();
        let mut field_foreign_keys: Vec<ForeignKeySpec> = Vec::new();

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
                } else if meta.path.is_ident("scale") {
                    scale = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
                } else if meta.path.is_ident("created_at") {
                    managed = Some("CreatedAt");
                } else if meta.path.is_ident("updated_at") {
                    managed = Some("UpdatedAt");
                } else if meta.path.is_ident("soft_delete") {
                    is_soft_delete = true;
                } else if meta.path.is_ident("index") {
                    field_indexes.push(parse_index(&meta, Some("\0self"))?);
                } else if meta.path.is_ident("check") {
                    field_checks.push(parse_check(&meta, Some("\0self"))?);
                } else if meta.path.is_ident("foreign_key") {
                    field_foreign_keys.push(parse_foreign_key(&meta, Some("\0self"))?);
                } else {
                    return Err(meta.error(
                        "unknown option; expected `pk`, `rename`, `added_in`, `scale`, \
                         `created_at`, `updated_at`, `soft_delete`, `index`, `check` or \
                         `foreign_key`",
                    ));
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
        // Resolved against the *renamed* column, not the field ident, so a
        // `#[record(rename = "...")]` beside a check still points a form at
        // the name the wire carries.
        for mut spec in field_checks {
            if spec.column.as_deref() == Some("\0self") {
                spec.column = Some(column.clone());
            }
            checks.push(spec);
        }
        // Against the *renamed* column for the reason the checks above are: a
        // foreign key's columns are matched to the parent's primary key by
        // name, and the field ident is a name nothing outside this file uses.
        for mut spec in field_foreign_keys {
            for name in &mut spec.columns {
                if name == "\0self" {
                    name.clone_from(&column);
                }
            }
            foreign_keys.push(spec);
        }
        if is_pk {
            primary_key.push(column.clone());
        }
        if is_soft_delete {
            // Refused here rather than left to the builder, which takes one
            // name and would silently keep whichever came last. The span points
            // at the second field, which is the one to delete.
            if let Some(first) = &soft_delete {
                return Err(syn::Error::new_spanned(
                    field,
                    format!(
                        "a second `soft_delete` column; `{first}` already carries \
                         the retirement stamp and a table has one"
                    ),
                ));
            }
            soft_delete = Some(column.clone());
        }

        fields.push(FieldSpec {
            ident,
            ty: field.ty.clone(),
            column,
            primary_key: is_pk,
            added_in,
            scale,
            managed,
        });
    }

    validate(input, &fields, &primary_key, tenant.as_ref(), &indexes)?;
    let managed_stmts: Vec<TokenStream2> = fields
        .iter()
        .filter_map(|f| {
            let which = Ident::new(f.managed?, proc_macro2::Span::call_site());
            let name = &f.column;
            Some(quote! {
                builder = builder.managed_for(#name, ::slate_orm::Managed::#which);
            })
        })
        .collect();
    validate_relations(&fields, &relations)?;
    validate_checks(&fields, &checks)?;
    validate_foreign_keys(&fields, &foreign_keys)?;

    // --- code generation
    let column_stmts = fields.iter().map(|f| {
        let name = &f.column;
        let ty = &f.ty;
        // A decimal's scale is not in the type, so it cannot come from `Field`
        // the way the type and nullability do: it is a property of the column
        // and has to be written on the column. The builder method is separate
        // for the same reason — `column()` has nowhere to put it.
        if let Some(scale) = f.scale {
            return quote! {
                builder = if <#ty as ::slate_orm::Field>::NULLABLE {
                    builder.nullable_decimal_column(#name, #scale)
                } else {
                    builder.decimal_column(#name, #scale)
                };
            };
        }
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
    // The schema layer does the real checking — the column must be a nullable
    // `I64`, and it explains why in `TableBuilder::build` — so this passes the
    // name and lets that refusal stand rather than restating two rules in a
    // second place where they could drift apart.
    let soft_delete_stmt = soft_delete.as_ref().map(|name| {
        quote! { builder = builder.soft_delete(#name); }
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

    let check_stmts = checks.iter().map(|spec| {
        let name = &spec.name;
        let expr = &spec.predicate;
        let column = spec
            .column
            .as_ref()
            .map(|c| quote! { check = check.with_column(#c); });
        let message = spec
            .message
            .as_ref()
            .map(|m| quote! { check = check.with_message(#m); });
        // `CheckDef::new` takes any `Predicate` and the write path evaluates
        // whatever it is handed, but a check `--print-schema` cannot read back
        // is one no client can be told about. Pinned to `Expr` here so that is
        // a type error on the attribute, which is the argument `only_where`
        // makes above and the same fix.
        quote! {
            builder = builder.check({
                let mut check = ::slate_orm::CheckDef::new(#name, {
                    #ordinal_bindings
                    let predicate: ::slate_orm::Expr = #expr;
                    predicate
                });
                #column
                #message
                check
            });
        }
    });

    let foreign_key_stmts = foreign_keys.iter().map(|spec| {
        let name = &spec.name;
        let parent = &spec.parent;
        let columns = &spec.columns;
        let on_delete = spec.on_delete.as_ref().map(|action| {
            quote! {
                key = key.on_delete(::slate_orm::ReferentialAction::#action);
            }
        });
        // `table_id()` rather than `table().id()`: `table()` caches behind a
        // `OnceLock` and this code runs *inside* that lock's initialiser, so a
        // table referencing itself — `employee.manager_id -> employee`, the
        // ordinary self-reference — would re-enter it and deadlock. The derive
        // overrides `table_id()` with the literal from `#[record(id = N)]`, so
        // nothing is built to read it.
        quote! {
            builder = builder.foreign_key({
                let mut key = ::slate_orm::ForeignKeyDef::builder(
                    #name,
                    <#parent as ::slate_orm::Record>::table_id(),
                );
                #(key = key.column(#columns);)*
                #on_delete
                key
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

            // A `through` relationship emits a name and no behaviour. The
            // capability — two batched reads with the join rows as the
            // intermediate key set — is `load_related_through`, which needs
            // only the two `Related` impls the join table's own declarations
            // already provide. What cannot be inferred is which table stands
            // in the middle, so that is all this says.
            if let Some(join) = &spec.through {
                return quote! {
                    impl #impl_generics ::slate_orm::Through<#other>
                        for #self_ident #ty_generics #where_clause
                    {
                        type Join = #join;
                    }
                };
            }

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
                    // After the key, because the schema layer refuses a managed
                    // column that is *in* the key and can only see that once
                    // the key is set. Emitting these first would turn a real
                    // schema error into no error at all.
                    #(#managed_stmts)*
                    #tenant_stmt
                    #soft_delete_stmt
                    #(#index_stmts)*
                    #(#check_stmts)*
                    #(#foreign_key_stmts)*
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

            fn table_id() -> ::slate_orm::TableId {
                ::slate_orm::TableId(#table_id)
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
/// Refuse a check whose `column` names nothing, and two checks with one name.
///
/// Both are refusals the schema layer cannot make for this path. `column` is
/// checked against the table only by the TOML loader
/// (`slate-serverd/src/schema.rs`), because `TableBuilder` holds the string
/// and has no reason to resolve it; so a derived check with a mistyped
/// `column` would reach a form as a field name nothing renders beside —
/// silently, at the first refused write, which is the failure
/// `ledger/2026-09-19-a-check-that-names-its-field.md` records this field as
/// existing to prevent. Here it is a compile error with a span on the
/// attribute, which is strictly better than the startup refusal the TOML path
/// gets, and it is the same trade `validate_relations` makes for `local`.
///
/// The duplicate name `TableBuilder::build` does catch, as `DuplicateCheck` —
/// but at `Record::table()`, which this macro's own emission turns into a
/// panic. Catching it here makes it a compile error instead, and costs one
/// comparison over a list that is never long.
fn validate_checks(fields: &[FieldSpec], checks: &[CheckSpec]) -> syn::Result<()> {
    for (i, spec) in checks.iter().enumerate() {
        if let Some(column) = &spec.column
            && !fields.iter().any(|f| f.column == *column)
        {
            return Err(syn::Error::new(
                spec.span,
                format!(
                    "`column = \"{column}\"` names no column of this table. A check's \
                     column is what a form puts the message beside, so one pointing at \
                     nothing is worse than none: write the column's name after any \
                     `rename`, or leave `column` off if the check is about more than one"
                ),
            ));
        }
        if checks.iter().take(i).any(|c| c.name == spec.name) {
            return Err(syn::Error::new(
                spec.span,
                format!(
                    "two checks named `{}`; the name is how a refusal identifies which rule \
                     fired, so a duplicate makes one of them unnameable",
                    spec.name
                ),
            ));
        }
    }
    Ok(())
}

/// Refuse a foreign key naming a column this struct does not have, and two
/// keys sharing a name.
///
/// The column rule is the one worth the code. `ForeignKeyBuilder` holds the
/// strings and resolves them when the table is built, so a typo is a
/// `SchemaError` from `Record::table()` — which is at least loud. But the
/// *parent* side is checked later still, by `Catalog`, and a key whose own
/// columns do not exist never gets that far, so the message a reader sees
/// names the wrong layer. Refusing here puts a span on the attribute.
fn validate_foreign_keys(fields: &[FieldSpec], keys: &[ForeignKeySpec]) -> syn::Result<()> {
    for (i, spec) in keys.iter().enumerate() {
        for column in &spec.columns {
            if !fields.iter().any(|f| &f.column == column) {
                return Err(syn::Error::new(
                    spec.span,
                    format!(
                        "foreign key `{}` references column `{}`, which this struct does not \
                         have. Columns are matched after `rename`, so name the column rather \
                         than the field if the two differ",
                        spec.name, column
                    ),
                ));
            }
        }
        // A duplicated column within one key would encode a parent key with
        // the same value twice, which no primary key has. Caught here because
        // the builder takes the list verbatim and the mismatch would surface
        // as an arity error naming the parent.
        for (j, column) in spec.columns.iter().enumerate() {
            if spec.columns.iter().take(j).any(|c| c == column) {
                return Err(syn::Error::new(
                    spec.span,
                    format!(
                        "foreign key `{}` names column `{}` twice; the columns are matched \
                         against the parent's primary key in order, and a key has each of \
                         its columns once",
                        spec.name, column
                    ),
                ));
            }
        }
        if keys.iter().take(i).any(|k| k.name == spec.name) {
            return Err(syn::Error::new(
                spec.span,
                format!(
                    "two foreign keys named `{}`; the name is how a refusal identifies which \
                     reference failed, so a duplicate makes one of them unnameable",
                    spec.name
                ),
            ));
        }
    }
    Ok(())
}

fn validate_relations(fields: &[FieldSpec], relations: &[RelationSpec]) -> syn::Result<()> {
    for spec in relations {
        // A `through` relationship names no columns on either side: it is the
        // join table's two ordinary relationships that carry them, and this
        // one only says which table stands in the middle. So the column rules
        // below do not apply to it, and requiring `foreign` here would demand
        // a column that has no meaning on this attribute.
        if spec.through.is_some() {
            if spec.local.is_some() || spec.foreign.is_some() {
                return Err(syn::Error::new(
                    spec.span,
                    "`through` takes no `local` or `foreign`: the columns belong to the join                      table's own `has_many` and `belongs_to`, and naming them here would be                      a second place for them to disagree",
                ));
            }
            continue;
        }
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

// --- #[derive(Enum)] ------------------------------------------------------

/// Derive [`Field`](../slate_orm/trait.Field.html) for a fieldless enum,
/// stored as its variant name in a `Str` column.
///
/// ```ignore
/// #[derive(Enum)]
/// enum Payment {
///     Cash,
///     #[record(rename = "credit card")]
///     CreditCard,
/// }
/// ```
///
/// **Why the name and not an ordinal.** Both work and both have a failure mode,
/// and they are not equally likely. With an integer, *reordering* the variants
/// silently reinterprets every stored row — and reordering is something people
/// do by accident, alphabetising a list or inserting a variant in the middle,
/// with nothing in the diff to suggest it matters. With a name, *renaming* a
/// variant does the same damage, but a rename is deliberate and the compiler
/// forces you to visit every use site while you do it. So the name is the
/// choice that fails on the rarer, louder action; `rename` exists so that
/// changing the Rust spelling need not change the stored one.
///
/// The cost is stated rather than hidden: names are longer than ordinals in
/// every row and every index entry, and an index over the column sorts
/// alphabetically rather than by declaration order — so a `severity` column
/// ranges over `"critical" < "info" < "warning"`, which is not the order anyone
/// means. A column whose order matters should be an integer with its own
/// `Field` impl, and this derive is the wrong tool for it.
#[proc_macro_derive(Enum, attributes(record))]
pub fn derive_enum(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    expand_enum(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_enum(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "#[derive(Enum)] is for enums; a struct wants #[derive(Record)]",
        ));
    };
    if data.variants.is_empty() {
        // An uninhabited type cannot be read out of a column, so `from_value`
        // would have no arm to take and every write would be unreachable. A
        // compile error here beats a `match` with no arms later.
        return Err(syn::Error::new_spanned(
            input,
            "#[derive(Enum)] needs at least one variant: an empty enum has no value to store",
        ));
    }

    let mut idents = Vec::new();
    let mut names = Vec::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "#[derive(Enum)] stores a variant as its name, so a variant carrying data has \
                 nowhere to put it. Store the payload in its own column, or write the `Field` \
                 impl by hand",
            ));
        }
        let mut name = variant.ident.to_string();
        for attribute in &variant.attrs {
            if !attribute.path().is_ident("record") {
                continue;
            }
            let renamed: LitStr = attribute.parse_args_with(|input: ParseStream<'_>| {
                let key: Ident = input.parse()?;
                if key != "rename" {
                    return Err(syn::Error::new(
                        key.span(),
                        "the only variant attribute is `rename = \"...\"`",
                    ));
                }
                input.parse::<Token![=]>()?;
                input.parse()
            })?;
            name = renamed.value();
        }
        idents.push(variant.ident.clone());
        names.push(name);
    }

    // Two variants storing one name would make `from_value` pick whichever arm
    // came first and lose the other for ever — silently, since writing works.
    // Checked here rather than left to the `match`, whose unreachable-pattern
    // warning does not fire on string literals.
    let mut seen: Vec<&String> = names.iter().collect();
    seen.sort();
    for pair in seen.windows(2) {
        if let [a, b] = pair
            && a == b
        {
            return Err(syn::Error::new_spanned(
                input,
                format!("two variants both store `{a}`; a stored name has to identify one"),
            ));
        }
    }

    let self_ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let target = self_ident.to_string();

    Ok(quote! {
        impl #impl_generics ::slate_orm::Field for #self_ident #ty_generics #where_clause {
            const VALUE_TYPE: ::slate_orm::ValueType = ::slate_orm::ValueType::Str;
            const NULLABLE: bool = false;

            fn to_value(&self) -> ::slate_orm::Value {
                ::slate_orm::Value::Str(match self {
                    #(Self::#idents => #names,)*
                }.into())
            }

            fn from_value(
                value: &::slate_orm::Value,
            ) -> ::core::result::Result<Self, ::slate_orm::FieldError> {
                match value {
                    ::slate_orm::Value::Str(name) => match name.as_ref() {
                        #(#names => ::core::result::Result::Ok(Self::#idents),)*
                        // A name no variant claims is an error, never a
                        // default. A stored row written by a newer build --
                        // one variant ahead -- is exactly this case, and
                        // silently becoming the first variant would be a wrong
                        // answer that reads as a right one.
                        _ => ::core::result::Result::Err(
                            ::slate_orm::FieldError::OutOfRange { target: #target },
                        ),
                    },
                    ::slate_orm::Value::Null => ::core::result::Result::Err(
                        ::slate_orm::FieldError::UnexpectedNull {
                            expected: <Self as ::slate_orm::Field>::VALUE_TYPE,
                        },
                    ),
                    other => ::core::result::Result::Err(
                        ::slate_orm::FieldError::TypeMismatch {
                            expected: <Self as ::slate_orm::Field>::VALUE_TYPE,
                            found: other.type_name(),
                        },
                    ),
                }
            }
        }
    })
}

#[cfg(test)]
// The same pair `slate-kernel`'s own test modules allow: a fixture that will
// not parse and a refusal that does not arrive are both this suite failing,
// and `expect`/`panic` say so at the line rather than through a `Result` no
// caller reads.
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::expand_enum;

    /// What `expand_enum` says when it refuses, not merely that it refuses.
    ///
    /// The `compile_fail` doctests on `slate_orm::Enum` cannot make this
    /// distinction, and #295 found out the hard way: removing the
    /// variant-carrying-data check and the empty-enum check left both
    /// doctests still passing, because the *generated* code does not compile
    /// for those shapes either. The shape is refused either way — what the
    /// checks buy is the message, and a `compile_fail` block cannot read one.
    ///
    /// So the safety property was never at risk; the diagnostic was, and it is
    /// the entire reason these two checks exist. Their own comments say so:
    /// "a compile error here beats a `match` with no arms later".
    fn refusal(source: &str) -> String {
        let parsed = syn::parse_str(source).expect("the fixture itself must parse");
        match expand_enum(&parsed) {
            Ok(_) => panic!("expected a refusal, got generated code:\n{source}"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn a_variant_carrying_data_is_refused_by_name() {
        let said = refusal("enum Event { Opened, Closed(String) }");
        assert!(
            said.contains("carrying data"),
            "the message should say what is wrong, got: {said}"
        );
    }

    #[test]
    fn an_empty_enum_is_refused_by_name() {
        let said = refusal("enum Nothing {}");
        assert!(
            said.contains("at least one variant"),
            "the message should say what is wrong, got: {said}"
        );
    }

    /// The other two refusals the doctests cover, tested the same way so that
    /// all four Enum diagnostics are held to the same standard rather than
    /// only the two that happened to survive a mutation.
    #[test]
    fn two_variants_storing_one_name_are_refused_by_name() {
        let said = refusal("enum Payment { Cash, #[record(rename = \"Cash\")] Coins }");
        assert!(
            said.contains("Cash"),
            "the message should name the collision, got: {said}"
        );
    }

    #[test]
    fn a_struct_is_refused_by_name() {
        let said = refusal("struct Payment { kind: String }");
        assert!(
            said.contains("is for enums"),
            "the message should point at the right derive, got: {said}"
        );
    }
}
