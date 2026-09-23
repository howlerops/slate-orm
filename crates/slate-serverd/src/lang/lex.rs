//! Tokens.
//!
//! Deliberately tiny and deliberately not configurable: one string quote, one
//! comment style (none), no escapes beyond a doubled quote. A configuration
//! expression that needs more than this is one that should be a Rust policy
//! instead, and a lexer that grows to meet every such case is how a "small
//! config language" becomes a language.

use super::{LangError, LangResult};

/// A lexical token, with the byte offset it started at.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) at: usize,
    pub(crate) kind: Kind,
}

/// What a token is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// A bare word: a column name, a keyword or a function name. Keywords are
    /// recognised by the parser rather than here, because `AND` is a keyword
    /// in a predicate and a perfectly good column name in a scalar.
    Word(String),
    /// A single-quoted string, with `''` meaning one quote.
    Str(String),
    /// A numeric literal, kept as text. Which of `i64`, `u64` and `f64` it
    /// becomes is decided by the column opposite it, so converting here would
    /// be converting before the type is known.
    Number(String),
    /// `:principal` or `:tenant`; the name follows the colon.
    Placeholder(String),
    /// One of the fixed operators and delimiters.
    Punct(&'static str),
}

impl Kind {
    /// How to name this token in an error message.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Word(w) => format!("`{w}`"),
            Self::Str(_) => "a string literal".to_owned(),
            Self::Number(n) => format!("`{n}`"),
            Self::Placeholder(p) => format!("`:{p}`"),
            Self::Punct(p) => format!("`{p}`"),
        }
    }
}

/// Operators, longest first so `<=` is not read as `<` then `=`.
///
/// Order is load-bearing, which is why it is a table rather than a chain of
/// `if`s: a chain that tested `<` before `<=` would parse `size <= 3` as
/// `size < (= 3)` and fail somewhere else entirely.
const PUNCTUATION: &[&str] = &[
    "<>", "<=", ">=", "!=", "!~*", "!~", "~*", "~", "=", "<", ">", "(", ")", "[", "]", ",", "+",
    "-", "*", "/",
];

/// Split `source` into tokens.
pub(crate) fn tokenize(source: &str) -> LangResult<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let Some(&byte) = bytes.get(i) else { break };

        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        if byte == b'\'' {
            let (text, next) = string_literal(source, i)?;
            tokens.push(Token {
                at: i,
                kind: Kind::Str(text),
            });
            i = next;
            continue;
        }

        if byte.is_ascii_digit() {
            let start = i;
            while i < bytes.len()
                && bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
            {
                i += 1;
            }
            tokens.push(Token {
                at: start,
                kind: Kind::Number(source.get(start..i).unwrap_or_default().to_owned()),
            });
            continue;
        }

        if byte == b'_' || byte.is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len()
                && bytes
                    .get(i)
                    .is_some_and(|b| *b == b'_' || b.is_ascii_alphanumeric())
            {
                i += 1;
            }
            tokens.push(Token {
                at: start,
                kind: Kind::Word(source.get(start..i).unwrap_or_default().to_owned()),
            });
            continue;
        }

        if byte == b':' {
            let start = i;
            i += 1;
            let name_start = i;
            while i < bytes.len()
                && bytes
                    .get(i)
                    .is_some_and(|b| *b == b'_' || b.is_ascii_alphanumeric())
            {
                i += 1;
            }
            if i == name_start {
                return Err(LangError::new(
                    start,
                    "a `:` must be followed by a placeholder name; the ones this server knows are `:principal` and `:tenant`",
                ));
            }
            tokens.push(Token {
                at: start,
                kind: Kind::Placeholder(source.get(name_start..i).unwrap_or_default().to_owned()),
            });
            continue;
        }

        if let Some(punct) = PUNCTUATION
            .iter()
            .find(|p| source.get(i..).is_some_and(|rest| rest.starts_with(**p)))
        {
            tokens.push(Token {
                at: i,
                kind: Kind::Punct(punct),
            });
            i += punct.len();
            continue;
        }

        // A double quote is worth its own message: it is what somebody reaches
        // for first, and TOML has already eaten one level of quoting, so the
        // mistake looks like the file rather than the expression.
        let hint = if byte == b'"' {
            ". String literals use single quotes here, because the value is already inside a TOML string"
        } else {
            ""
        };
        return Err(LangError::new(
            i,
            format!("`{}` cannot appear in an expression{hint}", byte as char),
        ));
    }

    Ok(tokens)
}

/// Read a `'…'` literal starting at `start`, returning it and the offset past
/// the closing quote.
fn string_literal(source: &str, start: usize) -> LangResult<(String, usize)> {
    let bytes = source.as_bytes();
    let mut out = String::new();
    let mut i = start + 1;
    loop {
        let Some(&byte) = bytes.get(i) else {
            return Err(LangError::new(start, "this string literal is never closed"));
        };
        if byte == b'\'' {
            // `''` is one quote, as in SQL. A backslash escape was rejected:
            // the value is already inside a TOML string, so a backslash would
            // have to be doubled in the file to reach the parser at all, and
            // `\\n` in a config file is a bug waiting to be filed.
            if bytes.get(i + 1) == Some(&b'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            return Ok((out, i + 1));
        }
        // Copy whole characters rather than bytes so a multi-byte character
        // survives.
        let rest = source.get(i..).unwrap_or_default();
        let Some(character) = rest.chars().next() else {
            return Err(LangError::new(start, "this string literal is never closed"));
        };
        out.push(character);
        i += character.len_utf8();
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Kind> {
        tokenize(source)
            .unwrap_or_else(|e| panic!("{}", e.message))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn a_two_character_operator_is_not_split() {
        assert_eq!(
            kinds("a <= 1"),
            vec![
                Kind::Word("a".into()),
                Kind::Punct("<="),
                Kind::Number("1".into())
            ]
        );
        assert_eq!(
            kinds("a <> 1"),
            vec![
                Kind::Word("a".into()),
                Kind::Punct("<>"),
                Kind::Number("1".into())
            ]
        );
        assert_eq!(kinds("a ~* 'x'").get(1), Some(&Kind::Punct("~*")));
        assert_eq!(kinds("a !~ 'x'").get(1), Some(&Kind::Punct("!~")));
        assert_eq!(kinds("a !~* 'x'").get(1), Some(&Kind::Punct("!~*")));
    }

    #[test]
    fn a_doubled_quote_is_one_quote() {
        assert_eq!(kinds("'it''s'"), vec![Kind::Str("it's".into())]);
    }

    #[test]
    fn an_unclosed_string_says_so_at_its_opening_quote() {
        let error = tokenize("kind = 'oops").unwrap_err();
        assert_eq!(error.at, 7);
        assert!(error.message.contains("never closed"), "{}", error.message);
    }

    #[test]
    fn a_double_quote_names_the_single_quote_rule() {
        let error = tokenize("kind = \"oops\"").unwrap_err();
        assert!(error.message.contains("single quotes"), "{}", error.message);
    }

    #[test]
    fn a_bare_colon_names_the_placeholders_that_exist() {
        let error = tokenize("owner = : ").unwrap_err();
        assert!(error.message.contains(":principal"), "{}", error.message);
    }

    #[test]
    fn a_multibyte_string_keeps_its_characters_and_its_offsets() {
        assert_eq!(kinds("'héllo'"), vec![Kind::Str("héllo".into())]);
        // `'é'` is four bytes, so `=` starts at 5 and `x` at 7. Offsets are
        // byte offsets, which is what `LangError::render` expects.
        let tokens = tokenize("'é' = x").unwrap_or_default();
        assert_eq!(tokens.get(1).map(|t| t.at), Some(5));
        assert_eq!(tokens.get(2).map(|t| t.at), Some(7));
    }
}
