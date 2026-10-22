//! A small AST for SML, built from the parse trees of the real grammar. It
//! covers what Strategy Studio analyses and simulates; anything else becomes an
//! `Unsupported` node that is reported precisely instead of being ignored.

use crate::syntax::Rule;
use pest::iterators::Pair;
use pest::pratt_parser::{Assoc, Op, PrattParser};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

impl BinOp {
    fn from_symbol(s: &str) -> Option<Self> {
        Some(match s {
            "+" => Self::Add,
            "-" => Self::Sub,
            "*" => Self::Mul,
            "/" => Self::Div,
            "%" => Self::Rem,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            "==" => Self::Eq,
            "!=" => Self::Ne,
            "&&" => Self::And,
            "||" => Self::Or,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f64),
    Str(String),
    Ident(String),
    /// A duration in seconds (`2_days`, or a number followed by a unit).
    Dur(f64),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// `target = value`, or `target op= value` when the operator is present.
    Assign(Option<BinOp>, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Vec<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    /// `lhs unit`, e.g. `5 min`: multiplies by the unit's length in seconds.
    Unit(Box<Expr>, f64),
    Range(Box<Expr>, Box<Expr>, bool),
    List(Vec<Expr>),
    /// A construct the simulator does not model (reported with its source line).
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub line: usize,
    pub kind: StmtKind,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `let` / `static` / `const` declaration: name, type text, initialiser.
    Decl { name: String, ty: Option<String>, init: Option<Expr>, is_static: bool },
    Expr(Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    Loop(Vec<Stmt>),
    For(String, Expr, Vec<Stmt>),
    Return(Option<Expr>),
    Listen,
    Unsupported(String),
}

pub fn line_of(p: &Pair<Rule>) -> usize {
    p.as_span().start_pos().line_col().0
}

pub fn col_of(p: &Pair<Rule>) -> usize {
    p.as_span().start_pos().line_col().1
}

/// Length in seconds of the unit named by a duration suffix (`min`, `days`, ...).
pub fn unit_seconds(unit: &str) -> Option<f64> {
    let unit = unit.trim();
    if unit == "ms" {
        return Some(0.001);
    }
    Some(match unit.trim_end_matches('s') {
        "milli" | "millisec" | "millisecond" => 0.001,
        "" | "sec" | "second" => 1.0,
        "m" | "min" | "minute" => 60.0,
        "hr" | "hour" => 3600.0,
        "d" | "day" => 86_400.0,
        "w" | "week" => 604_800.0,
        "mo" | "month" => 2_592_000.0,
        "yr" | "year" => 31_536_000.0,
        _ => return None,
    })
}

fn unit_for_rule(rule: Rule, text: &str) -> f64 {
    match rule {
        Rule::milli => 0.001,
        Rule::second => 1.0,
        Rule::minute => 60.0,
        Rule::hour => 3600.0,
        Rule::day => 86_400.0,
        Rule::week => 604_800.0,
        Rule::month => 2_592_000.0,
        Rule::year => 31_536_000.0,
        _ => unit_seconds(text).unwrap_or(1.0),
    }
}

fn pratt() -> &'static PrattParser<Rule> {
    static P: OnceLock<PrattParser<Rule>> = OnceLock::new();
    P.get_or_init(|| {
        use Assoc::*;
        use Rule::*;
        // Same precedence table as the compiler (core/transpiler/src/ast/mod.rs).
        PrattParser::new()
            .op(Op::infix(assign, Right) | Op::infix(eq, Right) | Op::infix(inline_if, Right))
            .op(Op::postfix(milli)
                | Op::postfix(second)
                | Op::postfix(minute)
                | Op::postfix(hour)
                | Op::postfix(day)
                | Op::postfix(week)
                | Op::postfix(month)
                | Op::postfix(year))
            .op(Op::infix(math9, Left))
            .op(Op::infix(math8, Left))
            .op(Op::infix(math7, Left))
            .op(Op::infix(math6, Left))
            .op(Op::infix(math5, Left))
            .op(Op::infix(math4, Left))
            .op(Op::infix(math3, Left))
            .op(Op::infix(math2, Left))
            .op(Op::infix(math1, Left))
            .op(Op::prefix(prefix))
            .op(Op::infix(member, Left) | Op::postfix(indx) | Op::postfix(pack) | Op::postfix(temp))
            .op(Op::infix(scope, Left))
    })
}

fn parse_duration_literal(text: &str) -> Expr {
    let (digits, unit) = text.split_once('_').unwrap_or((text, "s"));
    match (digits.parse::<f64>(), unit_seconds(unit)) {
        (Ok(n), Some(u)) => Expr::Dur(n * u),
        _ => Expr::Unsupported(format!("duration `{text}`")),
    }
}

/// Builds an expression from an `expr` / `range` pair.
pub fn expr(pair: Pair<Rule>) -> Expr {
    if pair.as_rule() == Rule::range {
        let mut parts = pair.clone().into_inner();
        let lo = expr(parts.next().unwrap());
        let op = parts.next().unwrap();
        let hi = expr(parts.next().unwrap());
        return Expr::Range(Box::new(lo), Box::new(hi), op.as_rule() == Rule::range_inclusive);
    }
    pratt()
        .map_primary(|p| primary(p))
        .map_prefix(|op, rhs| match op.into_inner().next().map(|i| i.as_rule()) {
            Some(Rule::not) => Expr::Not(Box::new(rhs)),
            Some(Rule::neg) => Expr::Neg(Box::new(rhs)),
            _ => Expr::Unsupported("`ref` expressions".into()),
        })
        .map_postfix(|lhs, op| match op.as_rule() {
            Rule::indx => Expr::Index(Box::new(lhs), op.into_inner().map(expr).collect()),
            Rule::pack => Expr::Call(Box::new(lhs), op.into_inner().map(expr).collect()),
            Rule::temp => Expr::Unsupported("generic specialisation `<...>`".into()),
            r => Expr::Unit(Box::new(lhs), unit_for_rule(r, op.as_str())),
        })
        .map_infix(|lhs, op, rhs| {
            let (l, r) = (Box::new(lhs), Box::new(rhs));
            match op.as_rule() {
                Rule::eq => Expr::Assign(None, l, r),
                Rule::assign => {
                    let sym = op.as_str().trim_end_matches('=');
                    match BinOp::from_symbol(sym) {
                        Some(b) => Expr::Assign(Some(b), l, r),
                        None => Expr::Unsupported(format!("operator `{}`", op.as_str())),
                    }
                }
                Rule::inline_if => {
                    let mid = op.into_inner().next().map(expr).unwrap_or(Expr::Unsupported("?:".into()));
                    Expr::Cond(l, Box::new(mid), r)
                }
                Rule::member => match *r {
                    Expr::Ident(name) => Expr::Member(l, name),
                    _ => Expr::Unsupported("member access".into()),
                },
                Rule::scope => Expr::Unsupported("`::` paths".into()),
                _ => match BinOp::from_symbol(op.as_str()) {
                    Some(b) => Expr::Bin(b, l, r),
                    None => Expr::Unsupported(format!("operator `{}`", op.as_str())),
                },
            }
        })
        .parse(pair.into_inner())
}

pub fn primary(p: Pair<Rule>) -> Expr {
    match p.as_rule() {
        Rule::atom => primary(p.into_inner().next().unwrap()),
        Rule::num => p.as_str().parse().map(Expr::Num).unwrap_or(Expr::Unsupported("number".into())),
        Rule::duration => parse_duration_literal(p.as_str()),
        Rule::ident => Expr::Ident(p.as_str().trim().to_owned()),
        Rule::parn => expr(p.into_inner().next().unwrap()),
        Rule::vec => Expr::List(p.into_inner().map(|e| if e.as_rule() == Rule::vec { primary(e) } else { expr(e) }).collect()),
        Rule::double_quoted_string | Rule::quoted_string => {
            let t = p.as_str();
            Expr::Str(t[1..t.len() - 1].to_owned())
        }
        other => Expr::Unsupported(format!("{other:?}")),
    }
}

/// Builds the statements of a `block`-like pair (`block`, `event_block`, `state_block`, ...).
pub fn block(pair: Pair<Rule>) -> Vec<Stmt> {
    if pair.as_rule() == Rule::block {
        // `{% ... %}` embedded C++ is a `cpp_block` directly under `block`.
        if let Some(inner) = pair.clone().into_inner().next() {
            if inner.as_rule() == Rule::cpp_block {
                return vec![Stmt { line: line_of(&inner), kind: StmtKind::Unsupported("inline C++ blocks `{% %}`".into()) }];
            }
        }
    }
    pair.into_inner().flat_map(statement).collect()
}

/// Converts one statement-ish pair into zero or more statements.
pub fn statement(p: Pair<Rule>) -> Vec<Stmt> {
    let line = line_of(&p);
    let one = |kind| vec![Stmt { line, kind }];
    match p.as_rule() {
        Rule::statement | Rule::event_statement | Rule::proc_statement | Rule::statement_block | Rule::proc_statement_block => {
            p.into_inner().flat_map(statement).collect()
        }
        Rule::var | Rule::typed_var | Rule::statc | Rule::typed_static | Rule::cnst | Rule::typed_const | Rule::signal | Rule::static_signal
        | Rule::pub_var | Rule::pub_const | Rule::pub_signal => {
            declarations(p)
        }
        Rule::mutable => one(StmtKind::Unsupported("`mut` declarations".into())),
        Rule::expr => one(StmtKind::Expr(expr(p))),
        Rule::ret => one(StmtKind::Return(p.into_inner().next().map(expr))),
        Rule::listen => one(StmtKind::Listen),
        Rule::record => one(StmtKind::Unsupported("`record`".into())),
        Rule::emit => one(StmtKind::Unsupported("`emit`".into())),
        Rule::import => vec![],
        Rule::if_clouse => {
            let mut it = p.into_inner();
            let cond = expr(it.next().unwrap());
            let then = block(it.next().unwrap());
            let other = it.next().map(block).unwrap_or_default();
            one(StmtKind::If(cond, then, other))
        }
        Rule::while_loop => {
            let mut it = p.into_inner();
            let cond = expr(it.next().unwrap());
            one(StmtKind::While(cond, block(it.next().unwrap())))
        }
        Rule::just_loop => one(StmtKind::Loop(block(p.into_inner().next().unwrap()))),
        Rule::for_loop => {
            let mut it = p.into_inner();
            let (var, iter) = for_header(it.next().unwrap());
            one(StmtKind::For(var, iter, block(it.next().unwrap())))
        }
        Rule::inline_loop => {
            let mut it = p.into_inner();
            let body = Stmt { line, kind: StmtKind::Expr(expr(it.next().unwrap())) };
            let (var, iter) = for_header(it.next().unwrap());
            one(StmtKind::For(var, iter, vec![body]))
        }
        other => one(StmtKind::Unsupported(format!("{other:?}"))),
    }
}

pub fn for_header(p: Pair<Rule>) -> (String, Expr) {
    let mut it = p.into_inner();
    let var = it.next().unwrap().as_str().trim().to_owned();
    (var, expr(it.next().unwrap()))
}

fn declarations(p: Pair<Rule>) -> Vec<Stmt> {
    let is_static = matches!(p.as_rule(), Rule::statc | Rule::typed_static | Rule::static_signal);
    let mut out: Vec<Stmt> = vec![];
    for part in p.into_inner() {
        let line = line_of(&part);
        match part.as_rule() {
            Rule::ident => out.push(Stmt { line, kind: StmtKind::Decl { name: part.as_str().trim().to_owned(), ty: None, init: None, is_static } }),
            Rule::typ => {
                if let Some(Stmt { kind: StmtKind::Decl { ty, .. }, .. }) = out.last_mut() {
                    *ty = Some(part.as_str().trim().to_owned());
                }
            }
            Rule::expr | Rule::vec => {
                let value = if part.as_rule() == Rule::vec { primary(part) } else { expr(part) };
                if let Some(Stmt { kind: StmtKind::Decl { init, .. }, .. }) = out.last_mut() {
                    *init = Some(value);
                }
            }
            _ => {}
        }
    }
    out
}

/// Renders an expression back to SML-like text for diagrams and messages.
pub fn show(e: &Expr) -> String {
    match e {
        Expr::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", *n as i64) } else { format!("{n}") }
        }
        Expr::Str(s) => format!("\"{s}\""),
        Expr::Ident(s) => s.clone(),
        Expr::Dur(secs) => show_duration(*secs),
        Expr::Not(x) => format!("!{}", show(x)),
        Expr::Neg(x) => format!("-{}", show(x)),
        Expr::Bin(op, a, b) => format!("{} {} {}", show(a), op_symbol(*op), show(b)),
        Expr::Assign(None, a, b) => format!("{} = {}", show(a), show(b)),
        Expr::Assign(Some(op), a, b) => format!("{} {}= {}", show(a), op_symbol(*op), show(b)),
        Expr::Cond(c, a, b) => format!("{} ? {} : {}", show(c), show(a), show(b)),
        Expr::Member(a, m) => format!("{}.{m}", show(a)),
        Expr::Index(a, i) => format!("{}[{}]", show(a), i.iter().map(show).collect::<Vec<_>>().join(", ")),
        Expr::Call(a, args) => format!("{}({})", show(a), args.iter().map(show).collect::<Vec<_>>().join(", ")),
        Expr::Unit(a, secs) => format!("{} {}", show(a), unit_name(*secs)),
        Expr::Range(a, b, inc) => format!("{}..{}{}", show(a), if *inc { "=" } else { "" }, show(b)),
        Expr::List(v) => format!("{{{}}}", v.iter().map(show).collect::<Vec<_>>().join(", ")),
        Expr::Unsupported(s) => format!("<{s}>"),
    }
}

fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn unit_name(secs: f64) -> &'static str {
    match secs as u64 {
        0 => "ms",
        1 => "s",
        60 => "min",
        3600 => "hour",
        86_400 => "day",
        604_800 => "week",
        2_592_000 => "month",
        31_536_000 => "yr",
        _ => "units",
    }
}

pub fn show_duration(secs: f64) -> String {
    for (len, name) in [(31_536_000.0, "yr"), (2_592_000.0, "month"), (604_800.0, "week"), (86_400.0, "day"), (3600.0, "hour"), (60.0, "min"), (1.0, "s")] {
        if secs >= len && (secs / len).fract() == 0.0 {
            return format!("{} {name}", (secs / len) as i64);
        }
    }
    format!("{secs} s")
}

/// Calls `f` on `e` and every sub-expression.
pub fn walk_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match e {
        Expr::Not(a) | Expr::Neg(a) | Expr::Member(a, _) | Expr::Unit(a, _) => walk_expr(a, f),
        Expr::Bin(_, a, b) | Expr::Assign(_, a, b) | Expr::Range(a, b, _) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        Expr::Cond(a, b, c) => {
            walk_expr(a, f);
            walk_expr(b, f);
            walk_expr(c, f);
        }
        Expr::Index(a, args) | Expr::Call(a, args) => {
            walk_expr(a, f);
            args.iter().for_each(|x| walk_expr(x, f));
        }
        Expr::List(items) => items.iter().for_each(|x| walk_expr(x, f)),
        Expr::Num(_) | Expr::Str(_) | Expr::Ident(_) | Expr::Dur(_) | Expr::Unsupported(_) => {}
    }
}

/// Calls `f` on every statement, including those nested in `if`/loops.
pub fn walk_stmts(stmts: &[Stmt], f: &mut dyn FnMut(&Stmt)) {
    for s in stmts {
        f(s);
        match &s.kind {
            StmtKind::If(_, a, b) => {
                walk_stmts(a, f);
                walk_stmts(b, f);
            }
            StmtKind::While(_, b) | StmtKind::Loop(b) | StmtKind::For(_, _, b) => walk_stmts(b, f),
            _ => {}
        }
    }
}

/// Calls `f(expr, line)` on every top-level expression of the statements
/// (conditions, initialisers, expression statements), recursively through blocks.
pub fn walk_stmt_exprs(stmts: &[Stmt], f: &mut dyn FnMut(&Expr, usize)) {
    walk_stmts(stmts, &mut |s| match &s.kind {
        StmtKind::Decl { init: Some(e), .. } | StmtKind::Expr(e) | StmtKind::Return(Some(e)) => f(e, s.line),
        StmtKind::If(c, _, _) | StmtKind::While(c, _) => f(c, s.line),
        StmtKind::For(_, it, _) => f(it, s.line),
        _ => {}
    });
}
