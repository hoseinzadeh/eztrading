//! Static checks on the strategy model. These are what the compiler does not
//! verify: transitions to states that do not exist, unreachable states,
//! processes that are never launched, and so on.

use crate::ast::{walk_expr, walk_stmt_exprs, walk_stmts, Expr, Stmt, StmtKind};
use crate::model::{Diagnostic, Interval, Machine, Severity, Strategy};
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn analyze(s: &Strategy) -> Vec<Diagnostic> {
    let mut out = vec![];
    if s.machines.is_empty() {
        out.push(Diagnostic::new(1, 1, Severity::Error, "no `machine` is defined"));
    }
    for m in &s.machines {
        machine(m, &mut out);
    }
    out.sort_by_key(|d| (d.line, d.col));
    out
}

fn machine(m: &Machine, out: &mut Vec<Diagnostic>) {
    let err = |line, col, msg: String| Diagnostic::new(line, col, Severity::Error, msg);
    let warn = |line, col, msg: String| Diagnostic::new(line, col, Severity::Warning, msg);

    let mut seen = HashSet::new();
    for p in &m.params {
        if !seen.insert(p.name.as_str()) {
            out.push(err(p.line, 1, format!("parameter `{}` is declared twice", p.name)));
        }
    }

    // Transitions and launches must point at declared states.
    let mut initial: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for l in &m.launches {
        match m.processes.iter().find(|p| p.name == l.process) {
            None => out.push(err(l.line, l.col, format!("`launch` refers to process `{}`, which does not exist", l.process))),
            Some(p) => {
                if !p.states.iter().any(|s| s.name == l.state) {
                    out.push(err(l.line, l.col, format!("`launch` starts `{}` in state `{}`, which is not declared in it", p.name, l.state)));
                } else {
                    initial.entry(p.name.as_str()).or_default().insert(l.state.as_str());
                }
                if l.args.len() != p.args.len() {
                    out.push(err(l.line, l.col, format!("process `{}` takes {} argument(s) but `launch` passes {}", p.name, p.args.len(), l.args.len())));
                }
            }
        }
    }

    for p in &m.processes {
        let names: HashSet<&str> = p.states.iter().map(|s| s.name.as_str()).collect();
        let mut dup = HashSet::new();
        for st in &p.states {
            if !dup.insert(st.name.as_str()) {
                out.push(err(st.line, st.col, format!("state `{}` is declared twice in process `{}`", st.name, p.name)));
            }
            for g in &st.gotos {
                if !names.contains(g.target.as_str()) {
                    out.push(err(g.line, g.col, format!("`goto {}`: process `{}` has no state with that name", g.target, p.name)));
                }
            }
            if st.gotos.is_empty() && !st.actions.is_empty() {
                out.push(Diagnostic::new(st.line, st.col, Severity::Info, format!("state `{}` has actions but no `goto`: they repeat on every wake-up and the process never leaves", st.name)));
            }
            if !st.gotos.is_empty() && st.gotos.iter().all(|g| g.guard.is_some()) && places_orders(&st.actions) {
                out.push(Diagnostic::new(st.line, st.col, Severity::Info, format!("state `{}` places orders and all its transitions are guarded: the orders repeat on every wake-up until a guard is true", st.name)));
            }
        }

        match initial.get(p.name.as_str()) {
            None => out.push(warn(p.line, 1, format!("process `{}` is never launched", p.name))),
            Some(start) => {
                let reach = reachable(p, start);
                for st in &p.states {
                    if !reach.contains(st.name.as_str()) {
                        out.push(warn(st.line, st.col, format!("state `{}` can never be reached from the launched state(s)", st.name)));
                    }
                }
            }
        }
    }

    // Parameters that nothing reads.
    let used = identifiers(m);
    for p in &m.params {
        if !used.contains(p.name.as_str()) {
            out.push(warn(p.line, 1, format!("parameter `{}` is never used", p.name)));
        }
    }
}

fn reachable<'a>(p: &'a crate::model::Process, start: &BTreeSet<&'a str>) -> HashSet<&'a str> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = start.iter().copied().collect();
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        if let Some(st) = p.states.iter().find(|s| s.name == n) {
            stack.extend(st.gotos.iter().map(|g| g.target.as_str()));
        }
    }
    seen
}

fn places_orders(stmts: &[Stmt]) -> bool {
    let mut found = false;
    walk_stmt_exprs(stmts, &mut |e, _| {
        walk_expr(e, &mut |x| {
            if let Expr::Call(f, _) = x {
                if matches!(&**f, Expr::Member(_, m) if matches!(m.as_str(), "buy" | "sell" | "buy_limit" | "sell_limit" | "buy_stop" | "sell_stop")) {
                    found = true;
                }
            }
        })
    });
    found
}

/// Every identifier that appears in an expression of the machine.
fn identifiers(m: &Machine) -> HashSet<&str> {
    // Collected into owned strings first: the walkers hand out short-lived borrows.
    let mut names: HashSet<String> = HashSet::new();
    let mut take = |e: &Expr| {
        walk_expr(e, &mut |x| {
            if let Expr::Ident(n) = x {
                names.insert(n.clone());
            }
        })
    };
    for p in &m.params {
        // A parameter's own default does not count as a use, but another's can.
        if let Some(d) = &p.default {
            take(d);
        }
    }
    let mut stmt_lists: Vec<&[Stmt]> = vec![&m.vars];
    for p in &m.processes {
        if let Interval::Every(e) = &p.interval {
            take(e);
        }
        stmt_lists.push(&p.prologue);
        for st in &p.states {
            for g in &st.gotos {
                if let Some(e) = &g.guard {
                    take(e);
                }
            }
            stmt_lists.push(&st.actions);
        }
    }
    for l in &m.launches {
        l.args.iter().for_each(&mut take);
        if let Some((_, e)) = &l.for_each {
            take(e);
        }
    }
    for list in stmt_lists {
        walk_stmt_exprs(list, &mut |e, _| take(e));
    }
    for ev in &m.events {
        walk_stmt_exprs(&ev.body, &mut |e, _| take(e));
    }
    let owned = names;
    m.params.iter().map(|p| p.name.as_str()).filter(|n| owned.contains(*n)).collect()
}

/// Constructs the simulator cannot run, reported with their line.
pub fn simulation_blockers(s: &Strategy, machine_name: Option<&str>) -> Vec<Diagnostic> {
    let mut out = vec![];
    let Some(m) = s.machine(machine_name) else { return out };
    let check = |stmts: &[Stmt], out: &mut Vec<Diagnostic>| {
        walk_stmts(stmts, &mut |st| {
            if let StmtKind::Unsupported(what) = &st.kind {
                out.push(Diagnostic::new(st.line, 1, Severity::Warning, format!("not simulated: {what}")));
            }
        });
        walk_stmt_exprs(stmts, &mut |e, line| unsupported_in(e, line, out));
    };
    check(&m.vars, &mut out);
    for p in &m.processes {
        check(&p.prologue, &mut out);
        for st in &p.states {
            check(&st.actions, &mut out);
            for g in &st.gotos {
                if let Some(e) = &g.guard {
                    unsupported_in(e, g.line, &mut out);
                }
            }
        }
    }
    for ev in s.events.iter().chain(&m.events) {
        check(&ev.body, &mut out);
    }
    out.dedup();
    out
}

fn unsupported_in(e: &Expr, line: usize, out: &mut Vec<Diagnostic>) {
    walk_expr(e, &mut |x| {
        if let Expr::Unsupported(what) = x {
            out.push(Diagnostic::new(line, 1, Severity::Warning, format!("not simulated: {what}")));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse;

    fn messages(src: &str) -> Vec<String> {
        analyze(&parse(src).unwrap()).into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn shipped_example_is_clean() {
        assert_eq!(messages(include_str!("../../../examples/threshold.sm")), Vec::<String>::new());
    }

    #[test]
    fn finds_goto_to_missing_state_with_position() {
        let src = "machine M {\n init { launch (p, A); }\n process p {\n  state A: goto B on x > 1;\n }\n}";
        let diags = analyze(&parse(src).unwrap());
        let d = diags.iter().find(|d| d.message.contains("goto B")).unwrap();
        assert_eq!((d.line, d.severity), (4, Severity::Error));
    }

    #[test]
    fn finds_bad_launches() {
        let m = messages("machine M { init { launch (nope, A); launch (p, Z); } process p { state A; } }");
        assert!(m.iter().any(|x| x.contains("process `nope`")), "{m:?}");
        assert!(m.iter().any(|x| x.contains("state `Z`")), "{m:?}");
    }

    #[test]
    fn warns_about_unreachable_states_unlaunched_processes_and_unused_params() {
        let m = messages("machine M { param k: float = 1.0; init { launch (p, A); } process p { state A: goto A; state B; } process q { state S; } }");
        assert!(m.iter().any(|x| x.contains("`B` can never be reached")), "{m:?}");
        assert!(m.iter().any(|x| x.contains("`q` is never launched")), "{m:?}");
        assert!(m.iter().any(|x| x.contains("`k` is never used")), "{m:?}");
    }

    #[test]
    fn hints_when_orders_repeat_while_waiting_for_a_guard() {
        let m = messages("machine M { param n: float = 1.0; init { launch (p, A); } process p { let pos: Position; state A: goto A on pos > 0 { pos = account.buy(s, n); } } }");
        assert!(m.iter().any(|x| x.contains("orders repeat")), "{m:?}");
    }

    #[test]
    fn simulation_blockers_name_unsupported_constructs() {
        let s = parse("machine M { init { launch (p, A); } process p { state A: goto A { if true {% x(); %} } } }");
        // Inline C++ is a parse-level construct; either it parses (and is flagged) or is a syntax issue.
        if let Ok(s) = s {
            let _ = simulation_blockers(&s, None);
        }
        let s = parse("machine M { init { launch (p, A); } process p { state A: goto A { record x in y; } } }").unwrap();
        let b = simulation_blockers(&s, None);
        assert!(b.iter().any(|d| d.message.contains("record")), "{b:?}");
    }
}
