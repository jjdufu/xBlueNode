//! `FROM MATCH`, `SUM_BY_ENTITY` / `AVG_BY_ENTITY`, and `MATCH_KNOWLEDGE` token scanners
//! for `AxQL` surface syntax (`05-STEP27` §27.2).

use axpoint_domain::AppError;
use axpoint_domain::studio::LinkDirection;

use crate::axql::parser::{
    DEFAULT_MATCH_KNOWLEDGE_TOP_K, DesugarState, FN_MATCH_KNOWLEDGE, KnowledgeCallSpec,
    KnowledgeQueryArg, MAX_MATCH_KNOWLEDGE_TOP_K, MatchHopPat, MatchNodePat, MatchPathSpec,
    bad_axql, desugar_axql_tokens, extract_balanced_parens, is_ident_continue, is_ident_start,
    is_valid_ident, skip_ws,
};

pub(crate) fn parse_match_chain(
    chars: &[char],
    start: usize,
) -> Result<(MatchPathSpec, usize), AppError> {
    let mut nodes = Vec::new();
    let mut hops = Vec::new();
    let (first_node, mut cursor) = parse_match_vertex(chars, start)?;
    nodes.push(first_node);

    loop {
        let ws_cursor = skip_ws(chars, cursor);
        let Some(&ch) = chars.get(ws_cursor) else {
            cursor = ws_cursor;
            break;
        };
        if ch == ',' {
            return Err(bad_axql(
                "JOIN_PATH_UNGROUNDED",
                "disconnected comma pattern in FROM MATCH is forbidden; connect entities via an ontology edge",
            ));
        }
        if ch == '-' || ch == '<' {
            let (hop, after_hop) = parse_match_hop(chars, ws_cursor)?;
            let v_pos = skip_ws(chars, after_hop);
            let (next_node, after_node) = parse_match_vertex(chars, v_pos)?;
            hops.push(hop);
            nodes.push(next_node);
            cursor = after_node;
        } else {
            break;
        }
    }

    if hops.is_empty() {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "FROM MATCH requires at least one relationship hop '-[:EDGE]->'",
        ));
    }

    Ok((MatchPathSpec { nodes, hops }, cursor))
}

fn parse_match_vertex(chars: &[char], start: usize) -> Result<(MatchNodePat, usize), AppError> {
    if chars.get(start) != Some(&'(') {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "expected '(' starting MATCH vertex pattern",
        ));
    }
    let (inside, after_paren) = extract_balanced_parens(chars, start)?;
    let Some((raw_alias, raw_type)) = inside.split_once(':') else {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("MATCH vertex '({inside})' must declare '(alias:NodeType)'"),
        ));
    };
    let alias = raw_alias.trim();
    let type_name = raw_type.trim();
    if !is_valid_ident(alias) || !is_valid_ident(type_name) {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("invalid MATCH vertex pattern '({inside})'"),
        ));
    }
    Ok((
        MatchNodePat {
            alias: String::from(alias),
            type_name: String::from(type_name),
        },
        after_paren,
    ))
}

fn parse_match_hop(chars: &[char], start: usize) -> Result<(MatchHopPat, usize), AppError> {
    let (is_left_arrow, bracket_pos) = if chars.get(start) == Some(&'<') {
        if chars.get(start.saturating_add(1)) != Some(&'-') {
            return Err(bad_axql("JOIN_PATH_UNGROUNDED", "expected '<-[' in MATCH"));
        }
        (true, skip_ws(chars, start.saturating_add(2)))
    } else {
        (false, skip_ws(chars, start.saturating_add(1)))
    };

    if chars.get(bracket_pos) != Some(&'[') {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "MATCH hop must specify an explicit edge pattern '[:EDGE_TYPE]'",
        ));
    }
    let mut i = bracket_pos.saturating_add(1);
    let mut inside = String::new();
    let mut closed = false;
    while let Some(&c) = chars.get(i) {
        i = i.saturating_add(1);
        if c == ']' {
            closed = true;
            break;
        }
        inside.push(c);
    }
    if !closed {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "unclosed '[' in MATCH hop",
        ));
    }

    let tail_pos = skip_ws(chars, i);
    let (direction, after_tail) = if is_left_arrow {
        if chars.get(tail_pos) != Some(&'-') {
            return Err(bad_axql("JOIN_PATH_UNGROUNDED", "expected '<-[...]-'"));
        }
        (LinkDirection::In, tail_pos.saturating_add(1))
    } else {
        if chars.get(tail_pos) != Some(&'-') || chars.get(tail_pos.saturating_add(1)) != Some(&'>')
        {
            return Err(bad_axql(
                "JOIN_PATH_UNGROUNDED",
                "directed MATCH hop must end with '->' or use '<-[:EDGE]-'",
            ));
        }
        (LinkDirection::Out, tail_pos.saturating_add(2))
    };

    let Some((raw_edge_alias, raw_edge_type)) = inside.split_once(':') else {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            format!("MATCH edge '[{inside}]' must declare an ontology edge type '[:EDGE]'"),
        ));
    };
    let clean_alias = raw_edge_alias.trim();
    let edge_alias = if clean_alias.is_empty() {
        None
    } else if is_valid_ident(clean_alias) {
        Some(String::from(clean_alias))
    } else {
        return Err(bad_axql("SQL_REJECTED", "invalid edge alias in MATCH"));
    };
    let edge_type = raw_edge_type.trim();
    if !is_valid_ident(edge_type) {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            format!("invalid ontology edge type '{edge_type}'"),
        ));
    }

    Ok((
        MatchHopPat {
            edge_alias,
            edge_type: String::from(edge_type),
            direction,
        },
        after_tail,
    ))
}

pub(crate) fn desugar_by_entity_fn(
    chars: &[char],
    after_fn: usize,
    sentinel_fn: &str,
    out: &mut String,
) -> Result<usize, AppError> {
    let open_pos = skip_ws(chars, after_fn);
    if chars.get(open_pos) != Some(&'(') {
        return Err(bad_axql(
            "SQL_REJECTED",
            "expected '(' after *_BY_ENTITY operator",
        ));
    }
    let (raw_body, after_paren) = extract_balanced_parens(chars, open_pos)?;
    let (expr_part, alias_part) = split_top_level_by_keyword(&raw_body)?;
    let inner_desugared = desugar_inner_dollar_params(expr_part.trim())?;
    let clean_alias = alias_part.trim();
    if !is_valid_ident(clean_alias) {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("invalid entity alias '{clean_alias}' in BY clause"),
        ));
    }
    out.push_str(sentinel_fn);
    out.push('(');
    out.push_str(&inner_desugared);
    out.push_str(", '");
    out.push_str(clean_alias);
    out.push_str("')");
    Ok(after_paren)
}

fn split_top_level_by_keyword(body: &str) -> Result<(&str, &str), AppError> {
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut depth = 0_usize;
    let mut i = 0_usize;
    let mut last_by_split: Option<(usize, usize)> = None;

    while let Some(&(byte_pos, ch)) = chars.get(i) {
        if ch == '\'' || ch == '"' {
            let quote = ch;
            i = i.saturating_add(1);
            while let Some(&(_, c)) = chars.get(i) {
                i = i.saturating_add(1);
                if c == quote {
                    if chars.get(i).map(|p| p.1) == Some(quote) {
                        i = i.saturating_add(1);
                    } else {
                        break;
                    }
                }
            }
            continue;
        }
        if ch == '(' {
            depth = depth.saturating_add(1);
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && is_ident_start(ch) {
            let prev_ok = i == 0
                || chars
                    .get(i.saturating_sub(1))
                    .is_some_and(|p| p.1.is_whitespace() || p.1 == ')');
            let mut j = i;
            let mut w = String::new();
            while let Some(&(_, c)) = chars.get(j) {
                if is_ident_continue(c) {
                    w.push(c);
                    j = j.saturating_add(1);
                } else {
                    break;
                }
            }
            let next_ok = chars.get(j).is_some_and(|p| p.1.is_whitespace());
            if prev_ok && next_ok && w.eq_ignore_ascii_case("BY") {
                let end_byte = chars.get(j).map_or(body.len(), |p| p.0);
                last_by_split = Some((byte_pos, end_byte));
            }
            i = j;
            continue;
        }
        i = i.saturating_add(1);
    }

    let Some((by_start, by_end)) = last_by_split else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "SUM_BY_ENTITY / AVG_BY_ENTITY requires '<expr> BY <entity_alias>'",
        ));
    };
    let lhs = body.get(..by_start).unwrap_or("").trim();
    let rhs = body.get(by_end..).unwrap_or("").trim();
    if lhs.is_empty() || rhs.is_empty() {
        return Err(bad_axql(
            "SQL_REJECTED",
            "SUM_BY_ENTITY / AVG_BY_ENTITY missing expression or entity alias",
        ));
    }
    Ok((lhs, rhs))
}

fn desugar_inner_dollar_params(expr_src: &str) -> Result<String, AppError> {
    let mut dummy = DesugarState {
        source: None,
        knowledge_calls: Vec::new(),
    };
    desugar_axql_tokens(expr_src, &mut dummy)
}

pub(crate) fn desugar_match_knowledge(
    chars: &[char],
    after_fn: usize,
    state: &mut DesugarState,
    out: &mut String,
) -> Result<usize, AppError> {
    let open_pos = skip_ws(chars, after_fn);
    if chars.get(open_pos) != Some(&'(') {
        return Err(bad_axql(
            "SQL_REJECTED",
            "expected '(' after MATCH_KNOWLEDGE",
        ));
    }
    let (raw_args, after_paren) = extract_balanced_parens(chars, open_pos)?;
    let parts = split_comma_args(&raw_args);
    if parts.is_empty() || parts.len() > 3 {
        return Err(bad_axql(
            "SQL_REJECTED",
            "MATCH_KNOWLEDGE expects ([alias,] $question [, top_k = N])",
        ));
    }

    let mut target_alias: Option<String> = None;
    let mut query_arg: Option<KnowledgeQueryArg> = None;
    let mut top_k = DEFAULT_MATCH_KNOWLEDGE_TOP_K;

    for (idx, part) in parts.iter().enumerate() {
        let p = part.trim();
        if let Some((k, v)) = p.split_once('=') {
            if !k.trim().eq_ignore_ascii_case("top_k") {
                return Err(bad_axql(
                    "SQL_REJECTED",
                    format!("unknown MATCH_KNOWLEDGE option '{}'", k.trim()),
                ));
            }
            let parsed_k: usize = v.trim().parse().map_err(|_| {
                bad_axql(
                    "SQL_REJECTED",
                    "MATCH_KNOWLEDGE top_k must be a positive integer",
                )
            })?;
            if parsed_k == 0 || parsed_k > MAX_MATCH_KNOWLEDGE_TOP_K {
                return Err(bad_axql(
                    "LIMIT_OUT_OF_RANGE",
                    format!("MATCH_KNOWLEDGE top_k must be in 1..={MAX_MATCH_KNOWLEDGE_TOP_K}"),
                ));
            }
            top_k = parsed_k;
        } else if idx == 0 && parts.len() >= 2 && is_valid_ident(p) {
            target_alias = Some(String::from(p));
        } else if query_arg.is_none() {
            query_arg = Some(parse_knowledge_query_arg(p)?);
        } else {
            return Err(bad_axql(
                "SQL_REJECTED",
                "unexpected positional argument in MATCH_KNOWLEDGE",
            ));
        }
    }

    let Some(q_arg) = query_arg else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "MATCH_KNOWLEDGE requires a $param or string literal question",
        ));
    };
    let call_id = state.knowledge_calls.len();
    state.knowledge_calls.push(KnowledgeCallSpec {
        call_id,
        target_alias,
        query_arg: q_arg,
        top_k,
    });
    out.push_str(FN_MATCH_KNOWLEDGE);
    out.push('(');
    out.push_str(&call_id.to_string());
    out.push(')');
    Ok(after_paren)
}

fn split_comma_args(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut i = 0_usize;
    while let Some(&c) = chars.get(i) {
        if c == '\'' || c == '"' {
            let quote = c;
            cur.push(c);
            i = i.saturating_add(1);
            while let Some(&qc) = chars.get(i) {
                cur.push(qc);
                i = i.saturating_add(1);
                if qc == quote {
                    if chars.get(i) == Some(&quote) {
                        cur.push(quote);
                        i = i.saturating_add(1);
                    } else {
                        break;
                    }
                }
            }
            continue;
        }
        if c == ',' {
            parts.push(cur.trim().to_owned());
            cur.clear();
        } else {
            cur.push(c);
        }
        i = i.saturating_add(1);
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_owned());
    }
    parts
}

fn parse_knowledge_query_arg(raw: &str) -> Result<KnowledgeQueryArg, AppError> {
    if let Some(p) = raw.strip_prefix('$').or_else(|| raw.strip_prefix(':')) {
        if !is_valid_ident(p) {
            return Err(bad_axql(
                "SQL_REJECTED",
                format!("invalid MATCH_KNOWLEDGE parameter '{raw}'"),
            ));
        }
        return Ok(KnowledgeQueryArg::Param(String::from(p)));
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        let inner = raw
            .get(1..raw.len().saturating_sub(1))
            .unwrap_or("")
            .replace("''", "'");
        return Ok(KnowledgeQueryArg::Literal(inner));
    }
    Err(bad_axql(
        "SQL_REJECTED",
        "MATCH_KNOWLEDGE question argument must be a $param/:param or single-quoted string",
    ))
}
