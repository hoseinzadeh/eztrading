//! State-machine diagrams: a JSON graph plus Mermaid source.

use crate::ast::{self, show, Stmt, StmtKind};
use crate::model::{Interval, Machine, Process, Strategy};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Diagram {
    pub machines: Vec<MachineGraph>,
    /// Mermaid `flowchart` source (top to bottom) for the first machine (or the requested one).
    pub mermaid: String,
}

#[derive(Debug, Serialize)]
pub struct MachineGraph {
    pub name: String,
    pub processes: Vec<ProcessGraph>,
}

#[derive(Debug, Serialize)]
pub struct ProcessGraph {
    pub name: String,
    pub signature: String,
    pub initial: Vec<String>,
    pub states: Vec<StateNode>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Serialize)]
pub struct StateNode {
    pub name: String,
    pub actions: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub guard: Option<String>,
    /// 1-based position among the state's transitions (they are tried in order).
    pub priority: usize,
    pub line: usize,
}

pub fn diagram(s: &Strategy, machine: Option<&str>) -> Diagram {
    let machines: Vec<MachineGraph> = s
        .machines
        .iter()
        .map(|m| MachineGraph { name: m.name.clone(), processes: m.processes.iter().map(|p| process_graph(m, p)).collect() })
        .collect();
    // An unknown machine name falls back to the first machine (the file may have changed).
    let mermaid = match s.machine(machine).or_else(|| s.machine(None)) {
        Some(m) => mermaid_for(&machines[s.machines.iter().position(|x| x.name == m.name).unwrap()]),
        None => String::new(),
    };
    Diagram { machines, mermaid }
}

fn process_graph(m: &Machine, p: &Process) -> ProcessGraph {
    let args = p.args.iter().map(|(n, t)| format!("{n}: {t}")).collect::<Vec<_>>().join(", ");
    let every = match &p.interval {
        Interval::Continuous => "continuously".to_owned(),
        Interval::Every(e) => format!("every {}", show(e)),
    };
    let mut initial: Vec<String> = vec![];
    for l in m.launches.iter().filter(|l| l.process == p.name) {
        if !initial.contains(&l.state) {
            initial.push(l.state.clone());
        }
    }
    let states = p
        .states
        .iter()
        .map(|st| StateNode { name: st.name.clone(), actions: action_lines(&st.actions), line: st.line })
        .collect();
    let edges = p
        .states
        .iter()
        .flat_map(|st| {
            st.gotos.iter().enumerate().map(|(i, g)| Edge {
                from: st.name.clone(),
                to: g.target.clone(),
                guard: g.guard.as_ref().map(show),
                priority: i + 1,
                line: g.line,
            })
        })
        .collect();
    ProcessGraph { name: p.name.clone(), signature: format!("{}({args}) · {every}", p.name), initial, states, edges }
}

fn action_lines(stmts: &[Stmt]) -> Vec<String> {
    let mut lines = vec![];
    ast::walk_stmts(stmts, &mut |s| {
        let text = match &s.kind {
            StmtKind::Expr(e) => show(e),
            StmtKind::Decl { name, init: Some(e), .. } => format!("let {name} = {}", show(e)),
            StmtKind::Return(Some(e)) => format!("return {}", show(e)),
            StmtKind::Unsupported(w) => format!("<{w}>"),
            _ => return,
        };
        lines.push(text);
    });
    lines
}

/// Escapes text for a quoted Mermaid label (entity codes, so no markup can sneak in).
fn esc(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '"' => out.push_str("#quot;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '&' => out.push_str("#amp;"),
            '|' => out.push_str("#124;"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}

fn mermaid_for(m: &MachineGraph) -> String {
    let mut out = String::from("flowchart TB\n");
    out.push_str("  classDef initial stroke-width:3px;\n  classDef terminal stroke-dasharray:4 3;\n");
    for (pi, p) in m.processes.iter().enumerate() {
        out.push_str(&format!("  subgraph sg{pi}[\"{}\"]\n    direction TB\n", esc(&p.signature)));
        for init in &p.initial {
            out.push_str(&format!("    p{pi}_start((\" \")) --> p{pi}_{init}\n"));
        }
        for st in &p.states {
            let mut label = format!("<b>{}</b>", esc(&st.name));
            for a in st.actions.iter().take(4) {
                label.push_str(&format!("<br/>{}", esc(&shorten(a, 46))));
            }
            if st.actions.len() > 4 {
                label.push_str("<br/>…");
            }
            out.push_str(&format!("    p{pi}_{}[\"{label}\"]\n", st.name));
        }
        let multi = |from: &str| p.edges.iter().filter(|e| e.from == from).count() > 1;
        for e in &p.edges {
            if !p.states.iter().any(|s| s.name == e.to) {
                continue; // a goto to a missing state is reported by the analysis
            }
            let text = match (&e.guard, multi(&e.from)) {
                (Some(g), true) => format!("{}. {}", e.priority, shorten(g, 40)),
                (Some(g), false) => shorten(g, 40),
                (None, true) => format!("{}. otherwise", e.priority),
                (None, false) => String::new(),
            };
            if text.is_empty() {
                out.push_str(&format!("    p{pi}_{} --> p{pi}_{}\n", e.from, e.to));
            } else {
                out.push_str(&format!("    p{pi}_{} -->|\"{}\"| p{pi}_{}\n", e.from, esc(&text), e.to));
            }
        }
        for st in &p.states {
            let terminal = !p.edges.iter().any(|e| e.from == st.name);
            let initial = p.initial.contains(&st.name);
            if terminal {
                out.push_str(&format!("    class p{pi}_{} terminal;\n", st.name));
            }
            if initial {
                out.push_str(&format!("    class p{pi}_{} initial;\n", st.name));
            }
        }
        out.push_str("  end\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse;

    #[test]
    fn graph_matches_the_example() {
        let d = diagram(&parse(include_str!("../../../examples/threshold.sm")).unwrap(), None);
        let p = &d.machines[0].processes[0];
        assert_eq!(p.initial, ["WAIT"]);
        assert_eq!(p.states.len(), 4);
        let edges: Vec<_> = p.edges.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect();
        assert_eq!(edges, [("WAIT", "BUY"), ("BUY", "HOLD"), ("HOLD", "SELL"), ("SELL", "WAIT")]);
        assert_eq!(p.edges[0].guard.as_deref(), Some("Tickers[sym] < floor"));
        assert!(p.signature.starts_with("watch(sym: Symbol) · every 5 min"), "{}", p.signature);
    }

    #[test]
    fn mermaid_is_well_formed_and_escaped() {
        let d = diagram(&parse(include_str!("../../../examples/threshold.sm")).unwrap(), None);
        assert!(d.mermaid.starts_with("flowchart TB"));
        assert!(d.mermaid.contains("p0_WAIT -->|\"Tickers[sym] #lt; floor\"| p0_BUY"), "{}", d.mermaid);
        assert!(d.mermaid.contains("p0_start((\" \")) --> p0_WAIT"));
        // No raw angle brackets from user text: only our own <b> and <br/> tags.
        let stripped = d.mermaid.replace("<b>", "").replace("</b>", "").replace("<br/>", "").replace("-->", "");
        assert!(!stripped.contains('<') && !stripped.contains('>'), "{stripped}");
    }

    #[test]
    fn unknown_machine_falls_back_to_the_first() {
        let s = parse(include_str!("../../../examples/threshold.sm")).unwrap();
        let d = diagram(&s, Some("NoSuchMachine"));
        assert!(d.mermaid.starts_with("flowchart TB"));
    }

    #[test]
    fn multiple_transitions_are_numbered() {
        let s = parse(include_str!("../../../tests/example.sm")).unwrap();
        let d = diagram(&s, Some("TrailingStopWithProfit"));
        assert!(d.mermaid.contains("1. pos #gt; 0"), "{}", d.mermaid);
        assert!(d.mermaid.contains("2. pos == 0"), "{}", d.mermaid);
    }

    #[test]
    fn markup_in_labels_cannot_inject() {
        let s = parse("machine M { init { launch (p, A); } process p { state A: goto A on x < 1 { a = \"<img src=x onerror=alert(1)>\"; } } }").unwrap();
        let d = diagram(&s, None);
        assert!(!d.mermaid.contains("<img"), "{}", d.mermaid);
    }
}
