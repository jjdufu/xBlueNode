//! Pratt expression parser enforcing `MAX_EXPR_CHARS`, `MAX_AST_DEPTH`, and `MAX_CALL_ARGS` (`06-STEP05-06` §06.1.1).

use super::lexer::{Token, TokenKind, tokenize};
use super::{
    BinaryOp, Expr, ExprIssue, ExprValue, MAX_AST_DEPTH, MAX_CALL_ARGS, MAX_EXPR_CHARS, UnaryOp,
};

fn parse_err(column: usize, message: impl Into<String>) -> ExprIssue {
    ExprIssue {
        code: String::from("EXPR_INVALID"),
        message: message.into(),
        column: Some(column),
    }
}

/// Parses `input` into a validated [`Expr`] AST with 1-based column diagnostics.
///
/// # Errors
/// Returns [`ExprIssue`] (`EXPR_INVALID`) when `input` is empty, exceeds [`MAX_EXPR_CHARS`],
/// exceeds [`MAX_AST_DEPTH`], exceeds [`MAX_CALL_ARGS`], uses chained comparisons, or has syntax errors.
pub fn parse_str(input: &str) -> Result<Expr, ExprIssue> {
    if input.chars().count() > MAX_EXPR_CHARS {
        return Err(parse_err(
            MAX_EXPR_CHARS.saturating_add(1),
            format!("expression exceeds {MAX_EXPR_CHARS} character limit"),
        ));
    }
    let tokens = tokenize(input)?;
    let Some(last_span_end_col) = tokens.last().map(|t| t.span.column.saturating_add(1)) else {
        return Err(parse_err(1, "expression cannot be empty"));
    };

    let mut parser = PrattParser {
        tokens,
        pos: 0,
        eof_col: last_span_end_col,
    };
    let (expr, _) = parser.parse_bp(0, 1)?;
    if let Some(extra) = parser.peek() {
        return Err(parse_err(
            extra.span.column,
            format!("unexpected token '{}'", describe_token(&extra.kind)),
        ));
    }
    Ok(expr)
}

struct PrattParser {
    tokens: Vec<Token>,
    pos: usize,
    eof_col: usize,
}

impl PrattParser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn current_col(&self) -> usize {
        self.peek().map_or(self.eof_col, |t| t.span.column)
    }

    fn next_token(&mut self) -> Option<Token> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        tok
    }

    fn parse_bp(&mut self, min_bp: u8, call_depth: usize) -> Result<(Expr, usize), ExprIssue> {
        if call_depth > MAX_AST_DEPTH {
            return Err(parse_err(
                self.current_col(),
                format!("expression exceeds maximum AST depth {MAX_AST_DEPTH}"),
            ));
        }

        let (mut lhs, mut lhs_depth) = self.parse_prefix(call_depth)?;

        while let Some(tok) = self.peek() {
            let Some((op, l_bp, r_bp)) = infix_binding_power(&tok.kind) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            let op_col = tok.span.column;
            let _ = self.next_token();
            let (rhs, rhs_depth) = self.parse_bp(r_bp, call_depth.saturating_add(1))?;

            if is_cmp_op(op)
                && let Some(next_tok) = self.peek()
                && infix_binding_power(&next_tok.kind)
                    .is_some_and(|(next_op, _, _)| is_cmp_op(next_op))
            {
                return Err(parse_err(
                    next_tok.span.column,
                    "chained comparison operators are not allowed",
                ));
            }

            let combined_depth = lhs_depth.max(rhs_depth).saturating_add(1);
            if combined_depth > MAX_AST_DEPTH {
                return Err(parse_err(
                    op_col,
                    format!("expression exceeds maximum AST depth {MAX_AST_DEPTH}"),
                ));
            }
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
            lhs_depth = combined_depth;
        }

        Ok((lhs, lhs_depth))
    }

    fn parse_prefix(&mut self, call_depth: usize) -> Result<(Expr, usize), ExprIssue> {
        let col = self.current_col();
        let Some(tok) = self.next_token() else {
            return Err(parse_err(col, "unexpected end of expression"));
        };

        match tok.kind {
            TokenKind::Number(v) => Ok((Expr::Literal(ExprValue::Number(v)), 1)),
            TokenKind::Text(s) => Ok((Expr::Literal(ExprValue::Text(s)), 1)),
            TokenKind::Bool(b) => Ok((Expr::Literal(ExprValue::Bool(b)), 1)),
            TokenKind::Null => Ok((Expr::Literal(ExprValue::Null), 1)),
            TokenKind::Not => {
                let (inner, d) = self.parse_bp(5, call_depth.saturating_add(1))?;
                let next_d = d.saturating_add(1);
                if next_d > MAX_AST_DEPTH {
                    return Err(parse_err(
                        tok.span.column,
                        format!("expression exceeds maximum AST depth {MAX_AST_DEPTH}"),
                    ));
                }
                Ok((
                    Expr::Unary {
                        op: UnaryOp::Not,
                        expr: Box::new(inner),
                    },
                    next_d,
                ))
            }
            TokenKind::Minus => {
                let (inner, d) = self.parse_bp(11, call_depth.saturating_add(1))?;
                let next_d = d.saturating_add(1);
                if next_d > MAX_AST_DEPTH {
                    return Err(parse_err(
                        tok.span.column,
                        format!("expression exceeds maximum AST depth {MAX_AST_DEPTH}"),
                    ));
                }
                Ok((
                    Expr::Unary {
                        op: UnaryOp::Neg,
                        expr: Box::new(inner),
                    },
                    next_d,
                ))
            }
            TokenKind::LParen => {
                let (inner, d) = self.parse_bp(0, call_depth.saturating_add(1))?;
                match self.next_token() {
                    Some(close) if close.kind == TokenKind::RParen => Ok((inner, d)),
                    Some(other) => Err(parse_err(
                        other.span.column,
                        format!("expected ')', got '{}'", describe_token(&other.kind)),
                    )),
                    None => Err(parse_err(self.eof_col, "expected closing ')'")),
                }
            }
            TokenKind::Ident(name) => self.parse_ident_tail(name, tok.span.column, call_depth),
            other => Err(parse_err(
                tok.span.column,
                format!("unexpected token '{}'", describe_token(&other)),
            )),
        }
    }

    fn parse_ident_tail(
        &mut self,
        name: String,
        ident_col: usize,
        call_depth: usize,
    ) -> Result<(Expr, usize), ExprIssue> {
        if self.peek().is_some_and(|t| t.kind == TokenKind::Dot) {
            return self.parse_scoped_ref(&name, ident_col);
        }

        if matches!(name.as_str(), "prop" | "param" | "src" | "dst" | "self") {
            return Err(parse_err(
                ident_col,
                format!("reserved identifier '{name}' must be followed by '.<field>'"),
            ));
        }

        if self.peek().is_some_and(|t| t.kind == TokenKind::LParen) {
            let _ = self.next_token(); // consume '('
            let mut args = Vec::new();
            let mut max_child_depth = 0_usize;

            if !self.peek().is_some_and(|t| t.kind == TokenKind::RParen) {
                loop {
                    if args.len() >= MAX_CALL_ARGS {
                        return Err(parse_err(
                            self.current_col(),
                            format!("function '{name}' exceeds maximum {MAX_CALL_ARGS} arguments"),
                        ));
                    }
                    let (arg_expr, arg_depth) = self.parse_bp(0, call_depth.saturating_add(1))?;
                    max_child_depth = max_child_depth.max(arg_depth);
                    args.push(arg_expr);

                    if self.peek().is_some_and(|t| t.kind == TokenKind::Comma) {
                        let _ = self.next_token();
                    } else {
                        break;
                    }
                }
            }

            match self.next_token() {
                Some(close) if close.kind == TokenKind::RParen => {}
                Some(other) => {
                    return Err(parse_err(
                        other.span.column,
                        format!(
                            "expected ')' closing call to '{name}', got '{}'",
                            describe_token(&other.kind)
                        ),
                    ));
                }
                None => {
                    return Err(parse_err(
                        self.eof_col,
                        format!("expected ')' closing call to '{name}'"),
                    ));
                }
            }

            let total_depth = max_child_depth.saturating_add(1);
            if total_depth > MAX_AST_DEPTH {
                return Err(parse_err(
                    ident_col,
                    format!("expression exceeds maximum AST depth {MAX_AST_DEPTH}"),
                ));
            }
            return Ok((Expr::Call { name, args }, total_depth));
        }

        Ok((Expr::MetricRef(name), 1))
    }

    fn parse_scoped_ref(
        &mut self,
        prefix: &str,
        ident_col: usize,
    ) -> Result<(Expr, usize), ExprIssue> {
        let dot_tok = self.next_token();
        let dot_col = dot_tok.map_or(ident_col, |t| t.span.column);
        if prefix == "prop" {
            return match self.next_token() {
                Some(Token {
                    kind: TokenKind::Ident(attr),
                    ..
                }) => Ok((Expr::PropRef(attr), 1)),
                Some(other) => Err(parse_err(
                    other.span.column,
                    "expected attribute identifier after 'prop.'",
                )),
                None => Err(parse_err(
                    dot_col.saturating_add(1),
                    "expected attribute identifier after 'prop.'",
                )),
            };
        }
        if !matches!(prefix, "param" | "src" | "dst" | "self" | "target" | "edge") {
            return Err(parse_err(
                ident_col,
                format!("dot property access is only supported via 'prop.<attr>', got '{prefix}.'"),
            ));
        }

        let first_seg = match self.next_token() {
            Some(Token {
                kind: TokenKind::Ident(seg),
                ..
            }) => seg,
            Some(other) => {
                return Err(parse_err(
                    other.span.column,
                    format!("expected field identifier after '{prefix}.'"),
                ));
            }
            None => {
                return Err(parse_err(
                    dot_col.saturating_add(1),
                    format!("expected field identifier after '{prefix}.'"),
                ));
            }
        };

        if self.peek().is_some_and(|t| t.kind == TokenKind::Dot)
            && prefix != "param"
            && matches!(first_seg.as_str(), "prop" | "metric")
        {
            let second_dot = self.next_token();
            let second_col = second_dot.map_or(dot_col, |t| t.span.column);
            return match self.next_token() {
                Some(Token {
                    kind: TokenKind::Ident(leaf),
                    ..
                }) => Ok((Expr::PropRef(format!("{prefix}.{first_seg}.{leaf}")), 1)),
                Some(other) => Err(parse_err(
                    other.span.column,
                    format!("expected identifier after '{prefix}.{first_seg}.'"),
                )),
                None => Err(parse_err(
                    second_col.saturating_add(1),
                    format!("expected identifier after '{prefix}.{first_seg}.'"),
                )),
            };
        }

        Ok((Expr::PropRef(format!("{prefix}.{first_seg}")), 1))
    }
}

const fn is_cmp_op(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

const fn infix_binding_power(kind: &TokenKind) -> Option<(BinaryOp, u8, u8)> {
    match kind {
        TokenKind::Or => Some((BinaryOp::Or, 1, 2)),
        TokenKind::And => Some((BinaryOp::And, 3, 4)),
        TokenKind::Eq => Some((BinaryOp::Eq, 6, 7)),
        TokenKind::Ne => Some((BinaryOp::Ne, 6, 7)),
        TokenKind::Lt => Some((BinaryOp::Lt, 6, 7)),
        TokenKind::Le => Some((BinaryOp::Le, 6, 7)),
        TokenKind::Gt => Some((BinaryOp::Gt, 6, 7)),
        TokenKind::Ge => Some((BinaryOp::Ge, 6, 7)),
        TokenKind::Plus => Some((BinaryOp::Add, 8, 9)),
        TokenKind::Minus => Some((BinaryOp::Sub, 8, 9)),
        TokenKind::Star => Some((BinaryOp::Mul, 10, 11)),
        TokenKind::Slash => Some((BinaryOp::Div, 10, 11)),
        TokenKind::Percent => Some((BinaryOp::Mod, 10, 11)),
        TokenKind::Caret => Some((BinaryOp::Pow, 12, 11)),
        _ => None,
    }
}

fn describe_token(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Number(v) => v.to_string(),
        TokenKind::Text(s) => format!("'{s}'"),
        TokenKind::Bool(b) => b.to_string(),
        TokenKind::Null => String::from("null"),
        TokenKind::Ident(s) => s.clone(),
        TokenKind::Plus => String::from("+"),
        TokenKind::Minus => String::from("-"),
        TokenKind::Star => String::from("*"),
        TokenKind::Slash => String::from("/"),
        TokenKind::Percent => String::from("%"),
        TokenKind::Caret => String::from("^"),
        TokenKind::Eq => String::from("=="),
        TokenKind::Ne => String::from("!="),
        TokenKind::Lt => String::from("<"),
        TokenKind::Le => String::from("<="),
        TokenKind::Gt => String::from(">"),
        TokenKind::Ge => String::from(">="),
        TokenKind::And => String::from("and"),
        TokenKind::Or => String::from("or"),
        TokenKind::Not => String::from("not"),
        TokenKind::LParen => String::from("("),
        TokenKind::RParen => String::from(")"),
        TokenKind::Comma => String::from(","),
        TokenKind::Dot => String::from("."),
    }
}
