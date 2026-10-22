use super::{lines::Lines, *};
use crate::{ast::r#type::Path, *};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Expr {
    Ty(Type),                                               // int
    Num(Token),                                             // 1.0
    Opr(Token),                                             // +
    Ident(Token, Option<Box<Definition>>),                  // a
    Ref(Box<Expr>),                                         // (a+b)
    Paren(Box<Expr>),                                       // (a+b)
    Signal(Box<Expr>),                                      // a.b where b is a signal
    Indexed(Box<Expr>, Vec<Expr>, Option<Box<Definition>>), // a[b]
    Specialized(Box<Expr>, Vec<Expr>),                      // T<a,b>
    FnCall(Box<Expr>, Vec<Expr>, Option<Box<Definition>>),  // a(1,2)
    Str(Token),                                             // "a+b"
    Prefix(Token, Box<Expr>),                               // ! a
    Infix(Box<Expr>, Box<Expr>, Box<Expr>),                 // a + b      ---> (a) (+) (b)
    // a ? b : c  ---> (a) (b) (c)
    Clock(Token, Option<Token>),
    Vec(Vec<Expr>),                                         // [a, b, c]
    Unevaluated,
}

impl Default for Expr {
    fn default() -> Self {
        Self::Unevaluated
    }
}

impl Compile for Expr {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self> {
        use Expr::*;
        PRATT_PARSER
            .map_primary(|primary| {
                Ok(match primary.as_rule() {
                    Rule::primary_types => Expr::Ty(Type::Prim(token!(primary))),
                    Rule::typ_path => {
                        let p = Path::compile(primary, ns)?;
                        Expr::Ty(Type::Path(
                            p.clone(),
                            Path::final_def(&p, ns).map(|p| Box::new(p.clone())),
                        ))
                    }
                    Rule::typ => Ty(Type::compile(primary, ns)?),
                    Rule::num | Rule::duration => Num(token!(primary)),
                    Rule::quoted_string | Rule::double_quoted_string => {
                        Ident(token!(primary), None)
                    }
                    Rule::ident => Ident(
                        token!(primary.clone()),
                        ns.scope()
                            .borrow()
                            .find_def(&token!(primary))
                            .map(|d| Box::new(d.clone())),
                    ),
                    Rule::expr | Rule::atom => Expr::compile(primary, ns)?,
                    Rule::parn => Paren(Box::new(Expr::compile(primary, ns)?)),
                    Rule::assign
                    | Rule::math1
                    | Rule::math2
                    | Rule::math3
                    | Rule::math4
                    | Rule::math5
                    | Rule::math6
                    | Rule::math7
                    | Rule::math8
                    | Rule::math9
                    | Rule::eq => Opr(token!(primary)),
                    _ => unreachable!("{:?}", primary),
                })
            })
            .map_prefix(|op, rhs| {
                let rhs = rhs?;
                let rule = op.as_str();
                if rule == "ref" {
                    Ok(Expr::Ref(Box::new(rhs)))
                } else {
                    Ok(Prefix(
                        token!(op),
                        Box::new(if rhs.is_signal(ns) {
                            Expr::Signal(Box::new(rhs))
                        } else {
                            rhs
                        }),
                    ))
                }
            })
            .map_infix(|lhs, op, rhs| {
                let mut lhs = lhs?;
                let mut rhs = rhs?;
                let mut pushed = false;
                // Decide this before a scope for the member's type is pushed: inside it the names
                // of the left-hand side (`Tickers`, `sym`) can no longer be resolved.
                let lhs_is_signal = lhs.is_signal(ns);
                if op.as_rule() == Rule::member || op.as_rule() == Rule::scope {
                    if let Some(def) = lhs.def(ns) {
                        pushed = ns.new_unnamed_scope(&def);
                    }
                }
                let res = if op.as_rule() == Rule::inline_if {
                    if lhs.is_signal(ns) {
                        lhs = Signal(Box::new(lhs))
                    }
                    if rhs.is_signal(ns) {
                        rhs = Signal(Box::new(rhs))
                    }
                    let mut op = Expr::compile(op, ns)?;
                    if op.is_signal(ns) {
                        op = Expr::Signal(Box::new(op));
                    }
                    Infix(Box::new(lhs), Box::new(op), Box::new(rhs))
                } else if op.as_rule() == Rule::member && lhs_is_signal {
                    let mut arrow = true;
                    if let Ident(t, _) = &rhs {
                        if t.as_str() == "history" {
                            arrow = false;
                        }
                    }
                    if arrow {
                        Infix(
                            Box::new(lhs),
                            Box::new(Expr::Opr(Token::from("->"))),
                            Box::new(rhs),
                        )
                    } else {
                        Infix(
                            Box::new(lhs),
                            Box::new(Expr::Opr(Token::from("."))),
                            Box::new(rhs),
                        )
                    }
                } else {
                    if op.as_rule() != Rule::member && op.as_rule() != Rule::scope {
                        if lhs.is_signal(ns) {
                            lhs = Signal(Box::new(lhs))
                        }
                        if rhs.is_signal(ns) {
                            rhs = Signal(Box::new(rhs))
                        }
                    }
                    Infix(
                        Box::new(lhs),
                        Box::new(Expr::Opr(token!(op))),
                        Box::new(rhs),
                    )
                };
                if pushed {
                    ns.drop_unnamed_scope();
                }
                Ok(res)
            })
            .map_postfix(|lhs, op| {
                let lhs = lhs?;
                let rule = op.as_rule();
                let txt = match rule {
                    Rule::milli => "milliseconds",
                    Rule::second => "seconds",
                    Rule::minute => "minutes",
                    Rule::hour => "hours",
                    Rule::day => "days",
                    Rule::week => "weeks",
                    Rule::month => "months",
                    Rule::year => "years",
                    _ => "",
                };
                let mut items = vec![];
                let mut tokens = op.into_inner();
                while let Some(token) = tokens.next() {
                    let item = Expr::compile(token, ns)?;
                    if item.is_signal(ns) {
                        items.push(Expr::Signal(Box::new(item)))
                    } else {
                        items.push(item)
                    }
                }
                Ok(match rule {
                    Rule::indx => Indexed(
                        Box::new(lhs.clone()),
                        items,
                        lhs.def(ns).map(|d| Box::new(d)),
                    ),
                    Rule::pack => {
                        let def = lhs.def(ns);
                        if def.is_none() {
                            println!(
                                "Warning: no definition found for '{}'",
                                lhs.header().print("").trim()
                            );
                        }
                        FnCall(Box::new(lhs.clone()), items, def.map(|d| Box::new(d)))
                    }
                    Rule::temp => Specialized(Box::new(lhs), items),
                    Rule::milli
                    | Rule::second
                    | Rule::minute
                    | Rule::hour
                    | Rule::day
                    | Rule::month
                    | Rule::week
                    | Rule::year => FnCall(
                        Box::new(Ident(
                            Token::from(format!("duration::{}", txt).as_str()),
                            None,
                        )),
                        vec![lhs.clone()],
                        None,
                    ),
                    _ => unreachable!("{:?}", rule),
                })
            })
            .parse(token.into_inner())
    }

    fn header(&self) -> Lines {
        use Expr::*;
        let mut lines = Lines::new();
        match self {
            Ty(t) => t.header(),
            Num(n) => lines << n,
            Opr(o) => lines << o,
            Ref(r) => r.header(),
            Ident(i, _) => lines << i,
            Paren(p) => lines << "(" << p.header() << ")",
            Signal(p) => lines << "(*(" << p.header() << "))",
            Indexed(v, i, _) => v.header() << "[" << i.join(", ") << "]",
            Specialized(t, s) => t.header() << "<" << s.join(", ") << ">",
            FnCall(f, a, d) => {
                let mut args = a.clone();
                if let Some(def) = d {
                    if let Definition::Fn(func) = def.as_ref() {
                        if let Func::Event(_) = func {
                            args.push(Expr::Ident(Token::from("__current_process__"), None));
                        }
                    }
                }
                f.header() << "(" << args.join(", ") << ")"
            }
            Str(s) => lines << s,
            Prefix(t, e) => lines << t << e.header(),
            Infix(lhs, op, rhs) => {
                if let Expr::Opr(op) = op.as_ref() {
                    if op.as_str() == "." || op.as_str() == "::" || op.as_str() == "->" {
                        lines << lhs.header() << op << rhs.header()
                    } else if op.as_str() == "::" {
                        lines << lhs.header() << op << rhs.header()
                    } else {
                        lines << lhs.header() << " " << op << " " << rhs.header()
                    }
                } else {
                    lines << lhs.header() << " ? " << op.header() << " : " << rhs.header()
                }
            }
            Expr::Vec(vec) => {
                lines = lines << "{";
                for expr in vec {
                    lines = lines << expr.header() << ",";
                }
                lines << "}"
            },
            Expr::Clock(_, _) => todo!(),
            Expr::Unevaluated => todo!(),
        }
    }

    fn source(&self, _ns: &str) -> Lines {
        self.header()
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        use Expr::*;
        match self {
            Ty(t) => Ty(t.specialize(generics, ns)),
            Ref(e) => Ref(Box::new(e.specialize(generics, ns))),
            Paren(e) => Paren(Box::new(e.specialize(generics, ns))),
            Signal(e) => Signal(Box::new(e.specialize(generics, ns))),
            Indexed(e, idx, d) => Indexed(
                Box::new(e.specialize(generics, ns)),
                idx.iter()
                    .map(|e| e.clone().specialize(generics, ns))
                    .collect(),
                d,
            ),
            Specialized(e, tmp) => {
                if let Some(def) = e.def(ns) {
                    if let Definition::Model(st) | Definition::Module(st) = def {
                        assert!(
                            tmp.len() == st.temp.len(),
                            "The number of generic types unmatched"
                        );
                        for i in 0..st.temp.len() {
                            if let Ty(ty) = &tmp[i] {
                                generics.insert(st.temp[i].clone(), ty.clone());
                            } else {
                                panic!("Expected a type")
                            }
                        }
                        Specialized(Box::new(e.specialize(generics, ns)), tmp)
                    } else {
                        eprint!("Warning: the type cannot be specialized");
                        Specialized(e, tmp)
                    }
                } else {
                    eprint!("Warning: could not find definition");
                    Specialized(e, tmp)
                }
            }
            FnCall(e, idx, d) => FnCall(
                Box::new(e.specialize(generics, ns)),
                idx.iter()
                    .map(|e| e.clone().specialize(generics, ns))
                    .collect(),
                d,
            ),
            Prefix(t, e) => Prefix(t, Box::new(e.specialize(generics, ns))),
            Infix(lhs, op, rhs) => Infix(
                Box::new(lhs.specialize(generics, ns)),
                Box::new(op.specialize(generics, ns)),
                Box::new(rhs.specialize(generics, ns)),
            ),
            _ => self,
        }
    }

    fn goto_def(&self) -> Option<Token> {
        match self {
            Expr::Ident(_, Some(d))
            | Expr::Indexed(_, _, Some(d))
            | Expr::FnCall(_, _, Some(d)) => d.goto_def(),
            _ => unreachable!()
        }
    }
}

impl Expr {
    pub fn is_signal(&self, ns: &NameSpace) -> bool {
        match self {
            Expr::Signal(_) => true,
            Expr::Ident(id, _) => ns.find_def(id, |d| d.is_signal(ns)),
            Self::Indexed(e, v, _) => {
                let def = e.def(ns);
                if let Some(d) = &def {
                    let define = match d {
                        Definition::Model(_) | Definition::Module(_) => def.clone(),
                        Definition::GlobalVar(d) | Definition::Var(d) => d.ty.def(ns),
                        Definition::Arg(_, t) => t.def(ns),
                        Definition::Fn(f) => f.inner().ret.def(ns),
                        Definition::Enum(_) => todo!(),
                        Definition::Cpp(_) => todo!(),
                    };
                    if let Some(d) = &define {
                        if let Definition::Model(st) | Definition::Module(st) = d {
                            for (_, f) in &st.scope.defs {
                                if let Definition::Fn(Func::Index(idx)) = f {
                                    if idx.args.len() == v.len() {
                                        return idx.ret.is_signal(None, ns);
                                    }
                                }
                            }
                        }
                    }
                }
                def.map(|d| d.is_signal(ns)).unwrap_or(false)
            }
            Expr::FnCall(id, _, d) => {
                if let Some(def) = d {
                    def.is_signal(ns)
                } else {
                    id.is_signal(ns)
                }
            }
            Expr::Infix(lhs, op, rhs) => {
                if let Expr::Opr(o) = op.as_ref() {
                    if o.as_str() == "." || o.as_str() == "->" || o.as_str() == "::" {
                        let mut pushed = 0;
                        if let Some(d) = lhs.def(ns) {
                            if ns.new_unnamed_scope(&d) {
                                pushed += 1;
                            }
                        } else {
                            if ns.new_named_scope(lhs.header().print("").trim()) {
                                pushed += 1;
                            }
                        }
                        let res = rhs.def(ns).map(|d| d.is_signal(ns)).unwrap_or(false);
                        for _ in 0..pushed {
                            ns.drop_unnamed_scope();
                        }
                        return res;
                    }
                }
                false
            }
            _ => false,
        }
    }

    // pub fn is_param(&self, ns: &NameSpace) -> bool {
    //     self.def(ns)
    //         .map(|d| d.ty().map(|t| t.is_param()).unwrap_or(false))
    //         .unwrap_or(false)
    // }

    pub fn def(&self, ns: &NameSpace) -> Option<Definition> {
        match self {
            Self::Ident(id, d) => {
                if let Some(d) = d {
                    Some(d.as_ref().clone())
                } else {
                    ns.find_def(id, |def| Some(def.clone()))
                }
            }
            Self::Indexed(e, v, d) => {
                if let Some(d) = d {
                    Some(d.as_ref().clone())
                } else {
                    let def = e.def(ns);
                    if let Some(d) = &def {
                        if let Definition::Module(st) = d {
                            for (_, f) in &st.scope.defs {
                                if let Definition::Fn(Func::Index(idx)) = f {
                                    if idx.args.len() == v.len() {
                                        return Some(f.clone());
                                    }
                                }
                            }
                        }
                    }
                    def
                }
            }
            Self::FnCall(e, _, d) => {
                if let Some(d) = d {
                    Some(d.as_ref().clone())
                } else {
                    e.def(ns)
                }
            }
            // Self::Specialized(e, tmp) => {
            //     if let Some(def) = e.def(ns) {
            //         let mut generics = HashMap::new();
            //         if let Definition::Model(st) | Definition::Module(st) = def.as_ref() {
            //             assert!(tmp.len() == st.temp.len(), "The number of generic types unmatched");
            //             for i in 0..st.temp.len() {
            //                 if let Expr::Ty(ty) = &tmp[i] {
            //                     generics.insert(st.temp[i].clone(), ty.clone());
            //                 } else {
            //                     panic!("Expected a type")
            //                 }
            //             }
            //             e.clone().specialize(&mut generics, ns).def(ns)
            //         } else {
            //             eprint!("Warning: the type cannot be specialized");
            //             None
            //         }
            //     } else {
            //         eprint!("Warning: could not find definition");
            //         None
            //     }
            // }
            Self::Infix(lhs, op, rhs) => {
                if let Self::Opr(o) = op.as_ref() {
                    if o.as_str() == "." || o.as_str() == "->" || o.as_str() == "::" {
                        if let Some(def) = lhs.def(ns) {
                            let pushed = ns.new_unnamed_scope(&def);
                            let res = rhs.def(ns);
                            if pushed {
                                ns.drop_unnamed_scope();
                            }
                            return res;
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}
