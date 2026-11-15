//! Deterministic expression engine contracts, AST, static type checking, and evaluation DTOs (`06-STEP05-06` §06.1 & §06.6).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::DataType;
use crate::studio::{NodeTypeSpec, PropertyKind};

/// Deterministic, panic-free expression evaluator.
pub mod eval;
/// Lexical analyzer with 1-based column span diagnostics.
pub mod lexer;
/// Pratt expression parser.
pub mod parser;
/// Static type checker and identifier reference collector.
pub mod typecheck;

pub use typecheck::{CheckedExprInfo, collect_references, typecheck_detailed};

/// Maximum expression character length (`06-STEP05-06` §06.1.2).
pub const MAX_EXPR_CHARS: usize = 2_000;

/// Maximum AST nesting depth (`06-STEP05-06` §06.1.2).
pub const MAX_AST_DEPTH: usize = 64;

/// Maximum function call argument count (`06-STEP05-06` §06.1.2).
pub const MAX_CALL_ARGS: usize = 16;

/// Source character span (byte offsets + 1-based character column) for diagnostic reporting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExprSpan {
    /// Byte start index in the input string.
    pub start: usize,
    /// Byte end index in the input string.
    pub end: usize,
    /// 1-based Unicode scalar character column where the token/node starts.
    pub column: usize,
}

/// Binary operators ordered by Pratt binding power (`06-STEP05-06` §06.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BinaryOp {
    /// Three-valued logical `or` / `||` (binding power 1, left-associative).
    #[serde(rename = "OR")]
    Or,
    /// Three-valued logical `and` / `&&` (binding power 2, left-associative).
    #[serde(rename = "AND")]
    And,
    /// Equality `==` (binding power 4, non-associative).
    #[serde(rename = "EQ")]
    Eq,
    /// Inequality `!=` (binding power 4, non-associative).
    #[serde(rename = "NE")]
    Ne,
    /// Less-than `<` (binding power 4, non-associative).
    #[serde(rename = "LT")]
    Lt,
    /// Less-than-or-equal `<=` (binding power 4, non-associative).
    #[serde(rename = "LE")]
    Le,
    /// Greater-than `>` (binding power 4, non-associative).
    #[serde(rename = "GT")]
    Gt,
    /// Greater-than-or-equal `>=` (binding power 4, non-associative).
    #[serde(rename = "GE")]
    Ge,
    /// Addition `+` (binding power 5, left-associative).
    #[serde(rename = "ADD")]
    Add,
    /// Subtraction `-` (binding power 5, left-associative).
    #[serde(rename = "SUB")]
    Sub,
    /// Multiplication `*` (binding power 6, left-associative).
    #[serde(rename = "MUL")]
    Mul,
    /// Division `/` (binding power 6, left-associative).
    #[serde(rename = "DIV")]
    Div,
    /// Modulo `%` (binding power 6, left-associative).
    #[serde(rename = "MOD")]
    Mod,
    /// Exponentiation `^` (binding power 8, right-associative).
    #[serde(rename = "POW")]
    Pow,
}

impl BinaryOp {
    /// Canonical infix symbol for formatting [`Expr`].
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Or => "or",
            Self::And => "and",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Mod => "%",
            Self::Pow => "^",
        }
    }
}

/// Prefix unary operators (`06-STEP05-06` §06.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum UnaryOp {
    /// Three-valued logical negation `not` / `!` (binding power 3).
    #[serde(rename = "NOT")]
    Not,
    /// Arithmetic negation `-` (binding power 7).
    #[serde(rename = "NEG")]
    Neg,
}

/// Static expression type inferred during schema validation (`06-STEP05-06` §06.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ExprType {
    /// Finite IEEE-754 floating-point number (`f64`).
    #[serde(rename = "NUMBER")]
    Number,
    /// UTF-8 string value.
    #[serde(rename = "TEXT")]
    Text,
    /// Boolean truth value.
    #[serde(rename = "BOOL")]
    Bool,
    /// Polymorphic `null` literal compatible with any concrete [`ExprType`].
    #[serde(rename = "NULL")]
    Null,
}

impl ExprType {
    /// Maps a [`DataType`] on `PropertySpec` to its referenceable [`ExprType`], or `None` if unreferenceable (`06-STEP05-06` §06.1.2).
    #[must_use]
    pub const fn from_property_data_type(dt: DataType) -> Option<Self> {
        match dt {
            DataType::Int64 | DataType::Float64 | DataType::Decimal => Some(Self::Number),
            DataType::String => Some(Self::Text),
            DataType::Boolean => Some(Self::Bool),
            DataType::Date
            | DataType::Timestamp
            | DataType::Binary
            | DataType::Json
            | DataType::List
            | DataType::Struct => None,
        }
    }
}

/// Runtime value produced and consumed by the expression evaluator (`06-STEP05-06` §06.1.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExprValue {
    /// Finite `f64` number.
    Number(f64),
    /// String value.
    Text(String),
    /// Boolean value.
    Bool(bool),
    /// Missing / null value.
    Null,
}

impl ExprValue {
    /// Returns the [`ExprType`] tag of this runtime value.
    #[must_use]
    pub const fn expr_type(&self) -> ExprType {
        match self {
            Self::Number(_) => ExprType::Number,
            Self::Text(_) => ExprType::Text,
            Self::Bool(_) => ExprType::Bool,
            Self::Null => ExprType::Null,
        }
    }

    /// Converts this value into JSON while enforcing `A1` §7 (non-finite numbers become `Value::Null`).
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            Self::Number(v) => serde_json::Number::from_f64(*v)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Self::Text(s) => serde_json::Value::String(s.clone()),
            Self::Bool(b) => serde_json::Value::Bool(*b),
            Self::Null => serde_json::Value::Null,
        }
    }
}

/// Parsed expression abstract syntax tree (`06-STEP05-06` §06.1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// Literal constant (`number`, `'string'`, `true`, `false`, `null`).
    Literal(ExprValue),
    /// Metric identifier on the owning node (`<ident>`).
    MetricRef(String),
    /// Static attribute reference on the owning node (`prop.<ident>`).
    PropRef(String),
    /// Prefix unary operation (`not x`, `!x`, `-x`).
    Unary {
        /// Unary operator.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<Self>,
    },
    /// Infix binary operation.
    Binary {
        /// Binary operator.
        op: BinaryOp,
        /// Left-hand operand.
        left: Box<Self>,
        /// Right-hand operand.
        right: Box<Self>,
    },
    /// Built-in deterministic function call (`name(arg0, ...)`).
    Call {
        /// Canonical lowercase function name.
        name: String,
        /// Positional argument expressions (`<= 16`).
        args: Vec<Self>,
    },
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(ExprValue::Number(v)) => {
                if matches!(v.classify(), std::num::FpCategory::Zero) {
                    f.write_str("0")
                } else if matches!(v.fract().classify(), std::num::FpCategory::Zero)
                    && v.abs() < 1e15
                {
                    write!(f, "{v:.0}")
                } else {
                    write!(f, "{v}")
                }
            }
            Self::Literal(ExprValue::Text(s)) => {
                write!(f, "'{}'", s.replace('\'', "''"))
            }
            Self::Literal(ExprValue::Bool(b)) => write!(f, "{b}"),
            Self::Literal(ExprValue::Null) => f.write_str("null"),
            Self::MetricRef(name) => f.write_str(name),
            Self::PropRef(name) => write!(f, "prop.{name}"),
            Self::Unary {
                op: UnaryOp::Not,
                expr,
            } => write!(f, "(not {expr})"),
            Self::Unary {
                op: UnaryOp::Neg,
                expr,
            } => write!(f, "(-{expr})"),
            Self::Binary { op, left, right } => write!(f, "({left} {} {right})", op.symbol()),
            Self::Call { name, args } => {
                write!(f, "{name}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                f.write_str(")")
            }
        }
    }
}

/// Static typing environment constructed from a [`NodeTypeSpec`] (`06-STEP05-06` §06.1.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExprTypeEnv {
    /// Declared `METRIC` property names (always typed as [`ExprType::Number`]).
    pub metrics: BTreeSet<String>,
    /// Declared `ATTRIBUTE` property names mapped to their [`ExprType`] (`None` if unreferenceable `DataType`).
    pub properties: BTreeMap<String, Option<ExprType>>,
}

impl ExprTypeEnv {
    /// Builds an [`ExprTypeEnv`] from a [`NodeTypeSpec`].
    #[must_use]
    pub fn from_node_type(nt: &NodeTypeSpec) -> Self {
        let mut metrics = BTreeSet::new();
        let mut properties = BTreeMap::new();
        for p in &nt.properties {
            match p.kind {
                PropertyKind::Metric => {
                    metrics.insert(p.name.clone());
                }
                PropertyKind::Attribute => {
                    properties.insert(
                        p.name.clone(),
                        ExprType::from_property_data_type(p.data_type),
                    );
                }
            }
        }
        Self {
            metrics,
            properties,
        }
    }
}

/// Runtime evaluation environment constructed from a node's `metrics` and `properties` (`06-STEP05-06` §06.1.2).
#[derive(Debug, Clone, Default)]
pub struct EvalEnv {
    /// Snapshot numeric metrics on the node.
    pub metrics: BTreeMap<String, f64>,
    /// Static attribute values on the node (`prop.<name>`).
    pub properties: BTreeMap<String, ExprValue>,
}

impl EvalEnv {
    /// Builds an [`EvalEnv`] from a node's `metrics` map and JSON `properties` object.
    #[must_use]
    pub fn from_node(
        metrics: &BTreeMap<String, f64>,
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        let mut props = BTreeMap::new();
        for (k, v) in properties {
            if k.starts_with("__") {
                continue;
            }
            let ev = match v {
                serde_json::Value::Number(n) => n
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map_or(ExprValue::Null, ExprValue::Number),
                serde_json::Value::String(s) => ExprValue::Text(s.clone()),
                serde_json::Value::Bool(b) => ExprValue::Bool(*b),
                serde_json::Value::Null
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => ExprValue::Null,
            };
            props.insert(k.clone(), ev);
        }
        Self {
            metrics: metrics.clone(),
            properties: props,
        }
    }
}

/// Non-fatal mathematical domain warning emitted when an operation yields `Null` (`06-STEP05-06` §06.1.2 & §06.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EvalWarningCode {
    /// Division or modulo by zero (`/` or `%`).
    #[serde(rename = "DIV_BY_ZERO")]
    DivByZero,
    /// Invalid math domain (`sqrt(-1)`, `ln(0)`, `log10(-5)`, or non-finite overflow).
    #[serde(rename = "DOMAIN_ERROR")]
    DomainError,
}

/// Evaluation output pairing the resulting [`ExprValue`] with any [`EvalWarningCode`] diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalReport {
    /// Computed value (`Null` on domain/div-by-zero warning).
    pub value: ExprValue,
    /// Deduplicated warnings encountered during evaluation.
    pub warnings: Vec<EvalWarningCode>,
}

/// Parse or static type-check diagnostic item (`06-STEP05-06` §06.1 & §06.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExprIssue {
    /// Diagnostic error code (typically `"EXPR_INVALID"`).
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Optional 1-based character column where the error occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

/// Request body for `POST /api/xblue/expr/validate` (`06-STEP05-06` §06.6).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateExprRequest {
    /// Owning `NodeTypeSpec.name` whose metrics and attributes define the typing scope.
    pub type_name: String,
    /// Expression source text to parse and type-check.
    pub expression: String,
    /// Optional subgraph slug (defaults to `"main"`).
    #[serde(default)]
    pub subgraph: Option<String>,
}

/// Response body for `POST /api/xblue/expr/validate` (`06-STEP05-06` §06.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateExprResponse {
    /// `true` when `issues` is empty.
    pub valid: bool,
    /// Inferred return type when `valid == true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_type: Option<ExprType>,
    /// Collected syntax or type errors.
    pub issues: Vec<ExprIssue>,
}

/// Request body for `POST /api/xblue/expr/evaluate` (`06-STEP05-06` §06.6).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluateExprRequest {
    /// Target runtime `xb_nodes.id` supplying `metrics` and `properties` (read-only).
    pub node_id: Uuid,
    /// Expression source text to evaluate.
    pub expression: String,
}

/// Response body for `POST /api/xblue/expr/evaluate` (`06-STEP05-06` §06.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluateExprResponse {
    /// Evaluated JSON value (`number`, `string`, `bool`, or `null`).
    pub value: serde_json::Value,
    /// Runtime value type tag.
    pub value_type: ExprType,
    /// Warnings emitted during evaluation (`DIV_BY_ZERO`, `DOMAIN_ERROR`).
    pub warnings: Vec<EvalWarningCode>,
}

/// Parses an expression string into an [`Expr`] AST (`06-STEP05-06` §06.1.1).
///
/// # Errors
/// Returns [`ExprIssue`] if `input` exceeds [`MAX_EXPR_CHARS`], exceeds [`MAX_AST_DEPTH`],
/// exceeds [`MAX_CALL_ARGS`], or contains lexical/syntactic errors.
pub fn parse(input: &str) -> Result<Expr, ExprIssue> {
    parser::parse_str(input)
}

/// Statically type-checks `expr` against `env` (`06-STEP05-06` §06.1.2).
///
/// # Errors
/// Returns [`ExprIssue`] (`EXPR_INVALID`) on unknown identifiers, unreferenceable property types,
/// operator type mismatches, or invalid function signatures.
pub fn typecheck(expr: &Expr, env: &ExprTypeEnv) -> Result<ExprType, ExprIssue> {
    typecheck::typecheck_detailed(expr, env).map(|info| info.result_type)
}

/// Evaluates `expr` deterministically against `env` (`06-STEP05-06` §06.1.2).
#[must_use]
pub fn eval(expr: &Expr, env: &EvalEnv) -> EvalReport {
    eval::eval_ast(expr, env)
}

/// Validates `expression` against `node_type` and builds a [`ValidateExprResponse`].
#[must_use]
pub fn validate_for_node_type(node_type: &NodeTypeSpec, expression: &str) -> ValidateExprResponse {
    let env = ExprTypeEnv::from_node_type(node_type);
    match parse(expression) {
        Err(issue) => ValidateExprResponse {
            valid: false,
            inferred_type: None,
            issues: vec![issue],
        },
        Ok(ast) => match typecheck(&ast, &env) {
            Ok(inferred) => ValidateExprResponse {
                valid: true,
                inferred_type: Some(inferred),
                issues: Vec::new(),
            },
            Err(issue) => ValidateExprResponse {
                valid: false,
                inferred_type: None,
                issues: vec![issue],
            },
        },
    }
}

