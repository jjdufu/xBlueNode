//! Lexical analyzer for the `xBlueStudio` deterministic expression engine (`06-STEP05-06` §06.1.1).

use std::iter::Peekable;
use std::str::CharIndices;

use super::{ExprIssue, ExprSpan};

/// Token category produced by [`tokenize`].
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Finite floating-point numeric literal.
    Number(f64),
    /// Single-quoted UTF-8 string literal (`'...'` with `''` escaping).
    Text(String),
    /// Boolean literal (`true` or `false`).
    Bool(bool),
    /// `null` literal.
    Null,
    /// Identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    Ident(String),
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `^`
    Caret,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `and` or `&&`
    And,
    /// `or` or `||`
    Or,
    /// `not` or `!`
    Not,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `,`
    Comma,
    /// `.`
    Dot,
}

/// Positioned lexical token with 1-based character column span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token payload.
    pub kind: TokenKind,
    /// Source span in the original expression string.
    pub span: ExprSpan,
}

struct LexCursor<'a> {
    input_len: usize,
    iter: Peekable<CharIndices<'a>>,
    col: usize,
}

impl<'a> LexCursor<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input_len: input.len(),
            iter: input.char_indices().peekable(),
            col: 1,
        }
    }

    fn peek_char(&mut self) -> Option<char> {
        self.iter.peek().map(|&(_, ch)| ch)
    }

    fn peek_byte_pos(&mut self) -> usize {
        self.iter.peek().map_or(self.input_len, |&(pos, _)| pos)
    }

    fn bump(&mut self) -> Option<(usize, char)> {
        let next = self.iter.next();
        if next.is_some() {
            self.col = self.col.saturating_add(1);
        }
        next
    }
}

fn lex_err(column: usize, message: impl Into<String>) -> ExprIssue {
    ExprIssue {
        code: String::from("EXPR_INVALID"),
        message: message.into(),
        column: Some(column),
    }
}

/// Tokenizes `input` into a sequence of [`Token`]s with 1-based column diagnostics.
///
/// # Errors
/// Returns [`ExprIssue`] (`EXPR_INVALID`) on invalid characters, malformed or non-finite numbers,
/// single `=` / `|` / `&` operators, or unterminated string literals.
pub fn tokenize(input: &str) -> Result<Vec<Token>, ExprIssue> {
    let mut cursor = LexCursor::new(input);
    let mut tokens = Vec::new();

    while let Some(ch) = cursor.peek_char() {
        if ch.is_whitespace() {
            let _ = cursor.bump();
            continue;
        }
        let start = cursor.peek_byte_pos();
        let start_col = cursor.col;

        let kind = if ch.is_ascii_digit() {
            lex_number(&mut cursor, start_col)?
        } else if ch == '\'' {
            lex_string(&mut cursor, start_col)?
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            lex_ident_or_keyword(&mut cursor)
        } else {
            lex_operator(&mut cursor, start_col)?
        };

        let end = cursor.peek_byte_pos();
        tokens.push(Token {
            kind,
            span: ExprSpan {
                start,
                end,
                column: start_col,
            },
        });
    }

    Ok(tokens)
}

fn lex_number(cursor: &mut LexCursor<'_>, start_col: usize) -> Result<TokenKind, ExprIssue> {
    let mut buf = String::new();
    while let Some(ch) = cursor.peek_char() {
        if ch.is_ascii_digit() {
            buf.push(ch);
            let _ = cursor.bump();
        } else {
            break;
        }
    }

    if cursor.peek_char() == Some('.') {
        buf.push('.');
        let _ = cursor.bump();
        let mut frac_digits = 0_usize;
        while let Some(ch) = cursor.peek_char() {
            if ch.is_ascii_digit() {
                buf.push(ch);
                let _ = cursor.bump();
                frac_digits = frac_digits.saturating_add(1);
            } else {
                break;
            }
        }
        if frac_digits == 0 {
            return Err(lex_err(
                start_col,
                format!("malformed number literal '{buf}': expected digits after '.'"),
            ));
        }
    }

    if matches!(cursor.peek_char(), Some('e' | 'E')) {
        buf.push('e');
        let _ = cursor.bump();
        if let Some(sign) = cursor.peek_char()
            && (sign == '+' || sign == '-')
        {
            buf.push(sign);
            let _ = cursor.bump();
        }
        let mut exp_digits = 0_usize;
        while let Some(ch) = cursor.peek_char() {
            if ch.is_ascii_digit() {
                buf.push(ch);
                let _ = cursor.bump();
                exp_digits = exp_digits.saturating_add(1);
            } else {
                break;
            }
        }
        if exp_digits == 0 {
            return Err(lex_err(
                start_col,
                format!("malformed exponent in number literal '{buf}'"),
            ));
        }
    }

    let parsed: f64 = buf
        .parse()
        .map_err(|_| lex_err(start_col, format!("invalid number literal '{buf}'")))?;
    if !parsed.is_finite() {
        return Err(lex_err(
            start_col,
            format!("non-finite number literal '{buf}'"),
        ));
    }
    let normalized = if matches!(parsed.classify(), std::num::FpCategory::Zero) {
        0.0
    } else {
        parsed
    };
    Ok(TokenKind::Number(normalized))
}

fn lex_string(cursor: &mut LexCursor<'_>, start_col: usize) -> Result<TokenKind, ExprIssue> {
    let _ = cursor.bump(); // consume opening '\''
    let mut out = String::new();
    loop {
        let Some((_, ch)) = cursor.bump() else {
            return Err(lex_err(start_col, "unterminated string literal"));
        };
        if ch == '\'' {
            if cursor.peek_char() == Some('\'') {
                let _ = cursor.bump();
                out.push('\'');
                continue;
            }
            return Ok(TokenKind::Text(out));
        }
        out.push(ch);
    }
}

fn lex_ident_or_keyword(cursor: &mut LexCursor<'_>) -> TokenKind {
    let mut ident = String::new();
    while let Some(ch) = cursor.peek_char() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            ident.push(ch);
            let _ = cursor.bump();
        } else {
            break;
        }
    }
    match ident.as_str() {
        "or" | "OR" => TokenKind::Or,
        "and" | "AND" => TokenKind::And,
        "not" | "NOT" => TokenKind::Not,
        "true" | "TRUE" => TokenKind::Bool(true),
        "false" | "FALSE" => TokenKind::Bool(false),
        "null" | "NULL" => TokenKind::Null,
        _ => TokenKind::Ident(ident),
    }
}

fn lex_operator(cursor: &mut LexCursor<'_>, start_col: usize) -> Result<TokenKind, ExprIssue> {
    let Some((_, ch)) = cursor.bump() else {
        return Err(lex_err(start_col, "unexpected end of expression"));
    };
    match ch {
        '+' => Ok(TokenKind::Plus),
        '-' => Ok(TokenKind::Minus),
        '*' => Ok(TokenKind::Star),
        '/' => Ok(TokenKind::Slash),
        '%' => Ok(TokenKind::Percent),
        '^' => Ok(TokenKind::Caret),
        '(' => Ok(TokenKind::LParen),
        ')' => Ok(TokenKind::RParen),
        ',' => Ok(TokenKind::Comma),
        '.' => Ok(TokenKind::Dot),
        '=' => {
            if cursor.peek_char() == Some('=') {
                let _ = cursor.bump();
                Ok(TokenKind::Eq)
            } else {
                Err(lex_err(
                    start_col,
                    "unexpected '='; use '==' for equality comparison",
                ))
            }
        }
        '!' => {
            if cursor.peek_char() == Some('=') {
                let _ = cursor.bump();
                Ok(TokenKind::Ne)
            } else {
                Ok(TokenKind::Not)
            }
        }
        '<' => {
            if cursor.peek_char() == Some('=') {
                let _ = cursor.bump();
                Ok(TokenKind::Le)
            } else {
                Ok(TokenKind::Lt)
            }
        }
        '>' => {
            if cursor.peek_char() == Some('=') {
                let _ = cursor.bump();
                Ok(TokenKind::Ge)
            } else {
                Ok(TokenKind::Gt)
            }
        }
        '|' => {
            if cursor.peek_char() == Some('|') {
                let _ = cursor.bump();
                Ok(TokenKind::Or)
            } else {
                Err(lex_err(start_col, "unexpected '|'; use '||' or 'or'"))
            }
        }
        '&' => {
            if cursor.peek_char() == Some('&') {
                let _ = cursor.bump();
                Ok(TokenKind::And)
            } else {
                Err(lex_err(start_col, "unexpected '&'; use '&&' or 'and'"))
            }
        }
        other => Err(lex_err(
            start_col,
            format!("unexpected character '{other}' in expression"),
        )),
    }
}
