//! The structure of a strategy: machines, parameters, processes, states and
//! transitions, extracted from the parse tree.

use crate::ast::{self, col_of, line_of, Expr, Stmt};
use crate::syntax::{Rule, SmlParser};
use pest::iterators::Pair;
use pest::Parser;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A problem tied to a position in the source (1-based line and column).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    pub fn new(line: usize, col: usize, severity: Severity, message: impl Into<String>) -> Self {
        Self { line, col, severity, message: message.into() }
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub default: Option<Expr>,
    pub label: Option<String>,
    pub options: Vec<(Expr, Option<String>)>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub enum Interval {
    /// No `every` clause, or `continuously`: wake on every update.
    Continuous,
    Every(Expr),
}

#[derive(Debug, Clone)]
pub struct Goto {
    pub target: String,
    pub guard: Option<Expr>,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct State {
    pub name: String,
    pub gotos: Vec<Goto>,
    pub actions: Vec<Stmt>,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct Process {
    pub name: String,
    pub args: Vec<(String, String)>,
    pub interval: Interval,
    /// Declarations and other statements outside states (run once, when the process starts).
    pub prologue: Vec<Stmt>,
    pub states: Vec<State>,
    pub listens: bool,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Launch {
    pub process: String,
    pub args: Vec<Expr>,
    pub state: String,
    pub for_each: Option<(String, Expr)>,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct EventDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub name: String,
    pub params: Vec<Param>,
    /// Machine-level variables (`let`, `static`, `const` outside processes).
    pub vars: Vec<Stmt>,
    pub processes: Vec<Process>,
    pub launches: Vec<Launch>,
    pub events: Vec<EventDef>,
    pub line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Strategy {
    pub machines: Vec<Machine>,
    /// Top-level `event`s, shared by every machine.
    pub events: Vec<EventDef>,
    pub globals: Vec<Stmt>,
    /// Constructs that were parsed but are ignored by the model.
    pub notes: Vec<Diagnostic>,
}

impl Strategy {
    pub fn machine(&self, name: Option<&str>) -> Option<&Machine> {
        match name {
            Some(n) => self.machines.iter().find(|m| m.name == n),
            None => self.machines.first(),
        }
    }
}

/// Parses SML source. A syntax error is returned as a diagnostic with its position.
pub fn parse(source: &str) -> Result<Strategy, Diagnostic> {
    let pairs = SmlParser::parse(Rule::code, source).map_err(|e| {
        let (line, col) = match e.line_col {
            pest::error::LineColLocation::Pos(p) | pest::error::LineColLocation::Span(p, _) => p,
        };
        Diagnostic::new(line, col, Severity::Error, syntax_message(&e, source, line, col))
    })?;
    let mut strategy = Strategy::default();
    for pair in pairs {
        match pair.as_rule() {
            Rule::code => {
                for item in pair.into_inner() {
                    top_level(item, &mut strategy);
                }
            }
            _ => top_level(pair, &mut strategy),
        }
    }
    Ok(strategy)
}

/// What a grammar rule means to a person reading an error message.
fn friendly(rule: Rule) -> &'static str {
    use Rule::*;
    match rule {
        math1 | math2 | math3 | math4 | math5 | math6 | math7 | math8 | math9 | eq | assign | member | scope | inline_if | infix => "an operator",
        indx => "`[`",
        pack | args => "`(`",
        temp => "`<`",
        ident | ident_atom | char => "a name",
        state_block => "`{` or `;`",
        process_body | model_body | block | event_block | block_with_launch => "`{`",
        expr | atom | prefix | not | neg | ref_ => "an expression",
        num => "a number",
        double_quoted_string | quoted_string => "a string",
        typ | typ_path | primary_types => "a type",
        goto => "`goto`",
        state => "`state`",
        interval => "`every`",
        display => "`as \"label\"`",
        for_stmt => "`for`",
        range | range_inclusive | range_exclusive => "a range",
        proc_statement | proc_statement_block | statement | statement_block | event_statement => "a statement",
        milli | second | minute | hour | day | week | month | year => "a time unit",
        EOI => "the end of the file",
        _ => "something else",
    }
}

fn syntax_message(e: &pest::error::Error<Rule>, source: &str, line: usize, col: usize) -> String {
    let found = source
        .lines()
        .nth(line.saturating_sub(1))
        .and_then(|l| l.chars().skip(col.saturating_sub(1)).collect::<String>().split_whitespace().next().map(|w| w.chars().take(16).collect::<String>()));
    let mut msg = String::from("syntax error");
    match &found {
        Some(w) => msg.push_str(&format!(": unexpected `{w}`")),
        None => msg.push_str(": unexpected end of line"),
    }
    if let pest::error::ErrorVariant::ParsingError { positives, .. } = &e.variant {
        let mut names: Vec<&str> = vec![];
        for r in positives {
            let n = friendly(*r);
            if n != "something else" && !names.contains(&n) {
                names.push(n);
            }
        }
        names.truncate(5);
        match names.len() {
            0 => {}
            1 => msg.push_str(&format!(", expected {}", names[0])),
            n => msg.push_str(&format!(", expected {} or {}", names[..n - 1].join(", "), names[n - 1])),
        }
    }
    if matches!(found.as_deref(), None | Some("}") | Some(")")) {
        msg.push_str(" (a `;` may be missing before this point)");
    }
    msg
}

fn top_level(item: Pair<Rule>, out: &mut Strategy) {
    match item.as_rule() {
        Rule::machine => out.machines.push(machine(item, &mut out.notes)),
        Rule::event => out.events.push(event(item)),
        Rule::import | Rule::EOI => {}
        Rule::func | Rule::module | Rule::cpp => out.notes.push(Diagnostic::new(
            line_of(&item),
            col_of(&item),
            Severity::Info,
            "functions, classes and top-level C++ are not modelled by Studio",
        )),
        _ => out.globals.extend(ast::statement(item)),
    }
}

fn event(p: Pair<Rule>) -> EventDef {
    let line = line_of(&p);
    let mut it = p.into_inner();
    let name = it.next().unwrap().as_str().trim().to_owned();
    let mut params = vec![];
    let mut body = vec![];
    for part in it {
        match part.as_rule() {
            Rule::args => params = args(part).into_iter().map(|(n, _)| n).collect(),
            Rule::event_block => body = ast::block(part),
            _ => {}
        }
    }
    EventDef { name, params, body, line }
}

fn args(p: Pair<Rule>) -> Vec<(String, String)> {
    p.into_inner()
        .filter(|a| a.as_rule() == Rule::arg)
        .map(|a| {
            let mut name = String::new();
            let mut ty = String::new();
            for part in a.into_inner() {
                match part.as_rule() {
                    Rule::ident => name = part.as_str().trim().to_owned(),
                    Rule::typ => ty = part.as_str().trim().to_owned(),
                    _ => {}
                }
            }
            (name, ty)
        })
        .collect()
}

fn machine(p: Pair<Rule>, notes: &mut Vec<Diagnostic>) -> Machine {
    let line = line_of(&p);
    let mut m = Machine { name: String::new(), params: vec![], vars: vec![], processes: vec![], launches: vec![], events: vec![], line };
    for part in p.into_inner() {
        match part.as_rule() {
            Rule::ident => m.name = part.as_str().trim().to_owned(),
            Rule::model_body => {
                for member in part.into_inner() {
                    match member.as_rule() {
                        Rule::param => m.params.extend(params(member)),
                        Rule::process => m.processes.push(process(member)),
                        Rule::launcher => launcher(member, &mut m.launches, notes),
                        Rule::event => m.events.push(event(member)),
                        Rule::func => notes.push(Diagnostic::new(line_of(&member), col_of(&member), Severity::Info, "`fn` is not modelled by Studio")),
                        _ => m.vars.extend(ast::statement(member)),
                    }
                }
            }
            _ => {}
        }
    }
    m
}

fn params(p: Pair<Rule>) -> Vec<Param> {
    let mut out: Vec<Param> = vec![];
    for part in p.into_inner() {
        let line = line_of(&part);
        match part.as_rule() {
            Rule::ident => out.push(Param { name: part.as_str().trim().to_owned(), ty: String::new(), default: None, label: None, options: vec![], line }),
            Rule::typ => {
                if let Some(last) = out.last_mut() {
                    last.ty = part.as_str().trim().to_owned();
                }
            }
            Rule::expr => {
                if let Some(last) = out.last_mut() {
                    last.default = Some(ast::expr(part));
                }
            }
            Rule::vec => {
                if let Some(last) = out.last_mut() {
                    last.default = Some(ast::primary(part));
                }
            }
            Rule::display => {
                if let Some(last) = out.last_mut() {
                    display(part, last);
                }
            }
            _ => {}
        }
    }
    out
}

fn unquote(p: &Pair<Rule>) -> String {
    let t = p.as_str();
    t[1..t.len() - 1].to_owned()
}

fn display(p: Pair<Rule>, param: &mut Param) {
    for part in p.into_inner() {
        match part.as_rule() {
            Rule::double_quoted_string => param.label = Some(unquote(&part)),
            Rule::options => {
                for item in part.into_inner() {
                    match item.as_rule() {
                        Rule::expr => param.options.push((ast::expr(item), None)),
                        Rule::double_quoted_string => {
                            if let Some(last) = param.options.last_mut() {
                                last.1 = Some(unquote(&item));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn launcher(p: Pair<Rule>, out: &mut Vec<Launch>, notes: &mut Vec<Diagnostic>) {
    for block in p.into_inner() {
        for item in block.into_inner() {
            if item.as_rule() != Rule::launch {
                notes.push(Diagnostic::new(line_of(&item), col_of(&item), Severity::Info, "only `launch` statements in `init` are modelled"));
                continue;
            }
            let (line, col) = (line_of(&item), col_of(&item));
            let mut launch = Launch { process: String::new(), args: vec![], state: String::new(), for_each: None, line, col };
            let mut idents = 0;
            for part in item.into_inner() {
                match part.as_rule() {
                    Rule::ident => {
                        if idents == 0 {
                            launch.process = part.as_str().trim().to_owned();
                        } else {
                            launch.state = part.as_str().trim().to_owned();
                        }
                        idents += 1;
                    }
                    Rule::pack => launch.args = part.into_inner().map(ast::expr).collect(),
                    Rule::for_stmt => launch.for_each = Some(ast::for_header(part)),
                    _ => {}
                }
            }
            out.push(launch);
        }
    }
}

fn process(p: Pair<Rule>) -> Process {
    let line = line_of(&p);
    let mut proc = Process { name: String::new(), args: vec![], interval: Interval::Continuous, prologue: vec![], states: vec![], listens: false, line };
    for part in p.into_inner() {
        match part.as_rule() {
            Rule::ident => proc.name = part.as_str().trim().to_owned(),
            Rule::args => proc.args = args(part),
            Rule::interval => proc.interval = interval(part),
            Rule::process_body => {
                for item in part.into_inner() {
                    let state_pair = if item.as_rule() == Rule::proc_statement_block {
                        item.clone().into_inner().next().filter(|i| i.as_rule() == Rule::state)
                    } else {
                        None
                    };
                    match state_pair {
                        Some(sp) => proc.states.push(state(sp)),
                        None => proc.prologue.extend(ast::statement(item)),
                    }
                }
            }
            _ => {}
        }
    }
    proc.listens = proc.prologue.iter().any(|s| matches!(s.kind, ast::StmtKind::Listen));
    proc
}

fn interval(p: Pair<Rule>) -> Interval {
    for part in p.into_inner() {
        match part.as_rule() {
            Rule::continual => return Interval::Continuous,
            Rule::expr => return Interval::Every(ast::expr(part)),
            Rule::milli | Rule::second | Rule::minute | Rule::hour | Rule::day | Rule::week | Rule::month | Rule::year => {
                let secs = ast::unit_seconds(part.as_str()).unwrap_or(1.0);
                return Interval::Every(Expr::Dur(secs));
            }
            _ => {}
        }
    }
    Interval::Continuous
}

fn state(p: Pair<Rule>) -> State {
    let (line, col) = (line_of(&p), col_of(&p));
    let mut st = State { name: String::new(), gotos: vec![], actions: vec![], line, col };
    for part in p.into_inner() {
        match part.as_rule() {
            Rule::ident => st.name = part.as_str().trim().to_owned(),
            Rule::goto => {
                let (line, col) = (line_of(&part), col_of(&part));
                let mut it = part.into_inner();
                let target = it.next().unwrap().as_str().trim().to_owned();
                let guard = it.next().map(ast::expr);
                st.gotos.push(Goto { target, guard, line, col });
            }
            Rule::state_block => st.actions = ast::block(part),
            _ => {}
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Strategy {
        parse(include_str!("../../../examples/threshold.sm")).unwrap()
    }

    #[test]
    fn extracts_machine_params_and_processes() {
        let s = example();
        let m = s.machine(None).unwrap();
        assert_eq!(m.name, "Threshold");
        let names: Vec<_> = m.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["equities", "account", "invest", "floor", "ceiling"]);
        assert_eq!(m.params[3].label.as_deref(), Some("Buy below"));
        assert_eq!(m.params[3].ty, "float");
        assert_eq!(m.processes.len(), 1);
        let p = &m.processes[0];
        assert_eq!(p.name, "watch");
        assert_eq!(p.args, vec![("sym".to_string(), "Symbol".to_string())]);
        assert!(p.listens);
        assert!(matches!(&p.interval, Interval::Every(Expr::Unit(_, secs)) if *secs == 60.0));
    }

    #[test]
    fn extracts_states_and_transitions() {
        let s = example();
        let p = &s.machines[0].processes[0];
        let states: Vec<_> = p.states.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(states, ["WAIT", "BUY", "HOLD", "SELL"]);
        let wait = &p.states[0];
        assert_eq!(wait.gotos.len(), 1);
        assert_eq!(wait.gotos[0].target, "BUY");
        assert_eq!(ast::show(wait.gotos[0].guard.as_ref().unwrap()), "Tickers[sym] < floor");
        assert!(p.states[1].gotos[0].guard.is_none());
        assert_eq!(p.states[1].actions.len(), 1);
    }

    #[test]
    fn extracts_launches() {
        let s = example();
        let l = &s.machines[0].launches[0];
        assert_eq!((l.process.as_str(), l.state.as_str()), ("watch", "WAIT"));
        assert_eq!(l.args.len(), 1);
        assert!(l.for_each.is_some());
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        let err = parse("machine M {\n  init { launch(main, INIT) }\n}").unwrap_err();
        assert_eq!(err.line, 2);
        assert_eq!(err.severity, Severity::Error);
        assert!(err.message.starts_with("syntax error: unexpected `}`"), "{}", err.message);
        assert!(err.message.contains("a `;` may be missing"), "{}", err.message);
        assert!(!err.message.contains("for_stmt"), "{}", err.message);
    }

    #[test]
    fn syntax_errors_use_plain_words_not_grammar_rule_names() {
        let err = parse("machine M {\n process p { state A: goto B of x; }\n}").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("unexpected `of`"), "{}", err.message);
        assert!(err.message.contains("an operator") || err.message.contains("`{`"), "{}", err.message);
        for raw in ["state_block", "process_body", "math1", "proc_statement"] {
            assert!(!err.message.contains(raw), "{}", err.message);
        }
    }

    #[test]
    fn parses_the_bigger_example_with_events() {
        let s = parse(include_str!("../../../tests/example.sm")).unwrap();
        assert_eq!(s.machines.len(), 2);
        assert_eq!(s.events.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["const_positive", "dropped"]);
        let tsp = s.machine(Some("TrailingStopWithProfit")).unwrap();
        assert_eq!(tsp.processes.len(), 3);
        let main = &tsp.processes[0];
        assert!(matches!(&main.interval, Interval::Every(Expr::Ident(n)) if n == "interval"));
        let enter = main.states.iter().find(|s| s.name == "ENTER").unwrap();
        assert_eq!(enter.gotos.len(), 2);
    }
}
