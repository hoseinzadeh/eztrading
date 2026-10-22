use super::*;
use crate::token;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Block {
    Block(Vec<Statement>),
    Cpp(Token),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Statement {
    Def(Var),         // let k = 12;
    Expr(Expr),       // a = 12 * foo(21)
    If(If),           // if a { b(); } else { c(); }
    Loop(Loop),       // loop { run(); }
    For(For),         // for i in 1..10 { print(i); }
    InlineFor(For),   // print(i) for i in 1..10;
    While(While),     // while i < 10 { i += 1; }
    State(State),     // state INIT: goto DONE when done() { ... }
    Match(Match),     // match expr { pat1 => exec1(), pat2 => exec2(), _ => panic() }
    Launch(Launch),   // launch (proc(arg), INIT);
    Block(Block),     // { ... }
    Ret(Expr),        // return a;
    Emit(Expr),       // emit a;
    Listen(Expr),     // listen a;
    Record(Expr, Expr),     // record a in "filename";
}

impl Default for Block {
    fn default() -> Self {
        Block::Block(Vec::new())
    }
}

impl Block {
    pub fn states(&self) -> Vec<State> {
        let mut res = Vec::new();
        if let Block::Block(block) = self {
            for line in block {
                if let Statement::State(state) = line {
                    res.push(state.clone())
                }
            }
        }
        res
    }

    pub fn stack(&self) -> Vec<Var> {
        let mut res = Vec::new();
        if let Self::Block(statements) = self {
            for st in statements {
                match st {
                    Statement::Def(var) => res.push(var.clone()),
                    Statement::Block(block) => res.append(&mut block.stack()),
                    Statement::If(f) => {
                        res.append(&mut f.r#if.stack());
                        if let Some(e) = &f.r#else {
                            res.append(&mut e.stack());
                        }
                    },
                    Statement::Loop(l) => res.append(&mut l.block.stack()),
                    Statement::For(f) | Statement::InlineFor(f) => res.append(&mut f.block.stack()),
                    Statement::While(w) => res.append(&mut w.block.stack()),
                    Statement::State(s) => res.append(&mut s.block.stack()),
                    Statement::Match(m) => for a in &m.arms {res.append(&mut a.block.stack())},
                    _ => {}
                }
            }
        }
        res
    }
}

impl Compile for Block {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut statements = vec![];
        while let Some(token) = tokens.next() {
            match token.as_rule() {
                Rule::cpp_block => return Ok(Self::Cpp(token!(token.into_inner().next().unwrap()))),
                Rule::statement | Rule::statement_block | Rule::proc_statement |
                Rule::proc_statement_block | Rule::event_statement => statements.append(
                    &mut Statement::compile(token, ns)?,
                ),
                Rule::launch => statements.push(Statement::Launch(Launch::compile(token, ns)?)),
                _ => unreachable!("{:?}", token),
            }
        }
        Ok(Self::Block(statements))
    }

    fn source(&self, ns: &str) -> Lines {
        match self {
            Block::Block(s) => s.iter().map(|st| st.source(ns)).collect::<Lines>(),
            Block::Cpp(b) => Lines::new() << b,
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        match self {
            Block::Block(b) => Block::Block(b.iter().map(|s| s.clone().specialize(generics, ns)).collect()),
            Block::Cpp(_) => self,
        }
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

impl Compile for Statement {
    type Output = Vec<Self>;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let token = tokens.next().unwrap();
        match token.as_rule() {
            Rule::cnst | Rule::statc | Rule::var | Rule::static_signal | Rule::signal | Rule::mutable => Ok(Var::compile(token, ns)?
                .iter()
                .map(|v| {
                    ns.scope().borrow_mut().defs.push((v.ident.clone(), Definition::Var(v.clone())));
                    Statement::Def(v.clone())
                })
                .collect()),
            Rule::ret => {
                let expr = Expr::compile(token.into_inner().next().unwrap(), ns)?;
                Ok(if let Expr::Ref(_) = expr {
                    vec![Statement::Ret(expr)]
                } else if expr.is_signal(ns) {
                    vec![Statement::Ret(Expr::Signal(Box::new(expr)))]
                } else {
                    vec![Statement::Ret(expr)]
                })
            },
            Rule::emit => Ok(vec![Statement::Emit(Expr::compile(
                token.into_inner().next().unwrap(),
                ns,
            )?)]),
            Rule::listen => {
                let mut list = vec![];
                let mut tokens = token.into_inner();
                while let Some(expr) = tokens.next() {
                    list.push(Statement::Listen(Expr::compile(expr, ns)?))
                }
                Ok(list)
            },
            Rule::record => {
                let mut tokens = token.into_inner();
                let expr = Expr::compile(tokens.next().unwrap(), ns)?;
                let file = Expr::compile(tokens.next().unwrap(), ns)?;
                Ok(vec![Statement::Record(expr, file)])
            },
            Rule::expr => Ok(vec![Statement::Expr(Expr::compile(token, ns)?)]),
            Rule::if_clouse => Ok(vec![Statement::If(If::compile(token, ns)?)]),
            Rule::inline_loop => Ok(vec![Statement::InlineFor(InlineFor::compile(token, ns)?)]),
            Rule::for_loop => Ok(vec![Statement::For(For::compile(token, ns)?)]),
            Rule::while_loop => Ok(vec![Statement::While(While::compile(token, ns)?)]),
            Rule::just_loop => Ok(vec![Statement::Loop(Loop::compile(token, ns)?)]),
            Rule::state => Ok(vec![Statement::State(State::compile(token, ns)?)]),
            _ => unreachable!("{:?}", token),
        }
    }

    fn source(&self, _ns: &str) -> Lines {
        use Statement::*;
        match self {
            Def(v) => v.source(""),
            Expr(e) => e.source("") << ";",
            If(i) => i.source(""),
            Loop(l) => l.source(""),
            InlineFor(f) |
            For(f) => f.source(""),
            While(w) => w.source(""),
            State(_) => Lines::new(), // Handled in State::source
            Match(_) => todo!(),
            Launch(l) => l.source(""),
            Block(b) => b.source(""),
            Ret(e) => Lines::new() << "return " << e.source("") << ";",
            Emit(e) => e.source("") << ".emit(__scheduler);",
            Listen(e) => e.source("") << ".add_listener(__current_process__);",
            Record(e, f) => e.source("") << ".attach_history(__current_process__, " << f.header() << ");",
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        use Statement::*;
        match self {
            Def(v) => Def(v.specialize(generics, ns)),
            Expr(e) => Expr(e.specialize(generics, ns)),
            If(i) => If(i.specialize(generics, ns)),
            Loop(l) => Loop(l.specialize(generics, ns)),
            For(f) => For(f.specialize(generics, ns)),
            InlineFor(f) => InlineFor(f.specialize(generics, ns)),
            While(w) => While(w.specialize(generics, ns)),
            State(s) => State(s.specialize(generics, ns)),
            Match(_) => todo!(),
            Launch(l) => Launch(l.specialize(generics, ns)),
            Block(b) => Block(b.specialize(generics, ns)),
            Ret(r) => Ret(r.specialize(generics, ns)),
            Emit(e) => Emit(e.specialize(generics, ns)),
            Listen(a) => Listen(a.specialize(generics, ns)),
            Record(a, f) => Record(a.specialize(generics, ns), f.specialize(generics, ns)),
        }
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct If {
    cond: Expr,
    r#if: Block,
    r#else: Option<Block>,
}

impl Compile for If {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut i = If::default();
        i.cond = Expr::compile(tokens.next().unwrap(), ns)?;
        i.r#if = ns.scoped(None, |_| {
            Block::compile(tokens.next().unwrap(), ns)
        })?;
        if let Some(block) = tokens.next() {
            i.r#else = Some(ns.scoped(None, |_| {
                Block::compile(block, ns)
            })?); 
        }
        Ok(i)
    }

    fn source(&self, _ns: &str) -> Lines {
        let lines = Lines::new() << "if (" << self.cond.source("") << ") {" << NewLine
            << self.r#if.source("") << NewLine
            << "}";
        if let Some(e) = &self.r#else {
            lines << " else {" << NewLine
                << e.source("") << NewLine
                << "}"
        } else {
            lines
        }
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.cond = self.cond.specialize(generics, ns);
        self.r#if = self.r#if.specialize(generics, ns);
        self.r#else = self.r#else.map(|e| e.specialize(generics, ns));
        self
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Loop {
    block: Block,
}

impl Compile for Loop {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        Ok(Loop {
            block: Block::compile(token.into_inner().next().unwrap(), ns)?
        })
    }

    fn source(&self, _ns: &str) -> Lines {
        Lines::new() << "while (true) {" << NewLine
            << self.block.source("") << NewLine
            << "}"
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.block = self.block.specialize(generics, ns);
        self
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ForStmt {
    var: Option<Var>,
    rang: Range,
}

impl Compile for ForStmt {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut f = ForStmt::default();
        f.var = Some(Var::for_var(token!(tokens.next().unwrap())));
        f.rang = Range::compile(tokens.next().unwrap(), ns)?;
        Ok(f)
    }

    fn source(&self, _ns: &str) -> Lines {
        let lines = Lines::new() << "for (auto ";
        let var = &self.var.as_ref().unwrap().ident;
        match &self.rang {
            Range::Exclusive(b, e) => 
                lines << var << " = " << b.source("") << "; " << var
                    << " < " << e.source("") << "; " << var << "++)",
            Range::Inclusive(b, e) => 
                lines << var << " = " << b.source("") << "; " << var
                    << " <= " << e.source("") << "; " << var << "++)",
            Range::Iterator(f) => 
                lines << "&" << var << " : " << f.source("") << ")",
            Range::Unevaluated => unreachable!(),
        }
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.rang = self.rang.specialize(generics, ns);
        self
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

pub struct InlineFor {}

impl Compile for InlineFor {
    type Output = For;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let expr = Expr::compile(tokens.next().unwrap(), ns)?;
        let mut f = For::default();
        f.stmt = ForStmt::compile(tokens.next().unwrap(), ns)?;
        let id = f.stmt.var.as_ref().unwrap().ident.clone();
        let var = Definition::Var(Var {
            ident: id.clone(),
            ty: if let Some(def) = f.stmt.rang.def(ns) {
                Type::Path(vec![], Some(Box::new(def.clone())))
            } else {
                Type::None
            },
            value: None,
            is_directive: false,
            val_is_signal: false,
            name: None,
            options: Vec::new()
        });
        f.block = ns.scoped(None, |scope| {
            scope.borrow_mut().defs.push((id, var));
            Block::Block(vec![Statement::Expr(expr)])
        });
        Ok(f)
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct For {
    stmt: ForStmt,
    block: Block,
}

impl Default for For {
    fn default() -> Self {
        Self {
            stmt: ForStmt {
                var: Default::default(),
                rang: Range::Unevaluated,
            },
            block: Default::default(),
        }
    }
}

impl Compile for For {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut f = For::default();
        f.stmt = ForStmt::compile(tokens.next().unwrap(), ns)?;
        let id = f.stmt.var.as_ref().unwrap().ident.clone();
        let var = Definition::Var(Var {
            ident: id.clone(),
            ty: if let Some(def) = f.stmt.rang.def(ns) {
                Type::Path(vec![], Some(Box::new(def.clone())))
            } else {
                Type::None
            },
            value: None,
            is_directive: false,
            val_is_signal: false,
            name: None,
            options: Vec::new()
        });
        f.block = ns.scoped(None, |scope| {
            scope.borrow_mut().defs.push((id, var));
            Block::compile(tokens.next().unwrap(), ns)
        })?;
        Ok(f)
    }

    fn source(&self, _ns: &str) -> Lines {
        Lines::new() << self.stmt.source("") << " {" << NewLine
            << self.block.source("") << NewLine
            << "}"
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.block = self.block.specialize(generics, ns);
        self.stmt = self.stmt.specialize(generics, ns);
        self
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Range {
    Exclusive(Expr, Expr),
    Inclusive(Expr, Expr),
    Iterator(Expr),
    Unevaluated,
}

impl Default for Range {
    fn default() -> Self {
        Range::Unevaluated
    }
}

impl Range {
    /// Return the type of the variable based on the expr in range
    pub fn def(&self, ns: &NameSpace) -> Option<Definition> {
        match self {
            Range::Exclusive(e, _) | Range::Inclusive(e, _) => e.def(ns),
            Range::Iterator(e) => {
                if let Some(def) = e.def(ns) {
                    let ty = def.ty();
                    if let Some(t) = ty {
                        if let Some(Type::Path(p, _)) = t.get_path() {
                            if !p.is_empty() {
                                if let Path::Spec(id, tv) = &p[0] {
                                    if id.as_str() == "Vec" {
                                        return Some(Definition::Var(Var {
                                            ident: Token::default(),
                                            ty: tv[0].clone(),
                                            value: None,
                                            is_directive: false,
                                            val_is_signal: false,
                                            name: None,
                                            options: Vec::new()
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
                unreachable!("Only a Vec<T> type can be used here {:?}", e.def(ns)) // add other containers, too
            }
            Range::Unevaluated => unreachable!(),
        }
    }
}

impl Compile for Range {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        match token.as_rule() {
            Rule::expr => Ok(Self::Iterator(Expr::compile(token, ns)?)),
            Rule::range => {
                let mut tokens = token.into_inner();
                let left = Expr::compile(tokens.next().unwrap(), ns)?;
                let opr = tokens.next().unwrap();
                let right = Expr::compile(tokens.next().unwrap(), ns)?;
                if opr.as_rule() == Rule::range_inclusive {
                    Ok(Self::Inclusive(left, right))
                } else {
                    Ok(Self::Exclusive(left, right))
                }
            }
            _ => unreachable!("{:?}", token),
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        use Range::*;
        match self {
            Exclusive(b, e) => Exclusive(b.specialize(generics, ns), e.specialize(generics, ns)),
            Inclusive(b, e) => Inclusive(b.specialize(generics, ns), e.specialize(generics, ns)),
            Iterator(i) => Iterator(i.specialize(generics, ns)),
            Unevaluated => self,
        }
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct While {
    cond: Expr,
    block: Block,
}

impl Compile for While {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut w = While::default();
        w.cond = Expr::compile(tokens.next().unwrap(), ns)?;
        w.block = ns.scoped(None, |_| {
            Block::compile(tokens.next().unwrap(), ns)
        })?;
        Ok(w)
    }

    fn source(&self, _ns: &str) -> Lines {
        Lines::new() << "while (" << self.cond.source("") << ") {" << NewLine
            << self.block.source("") << NewLine
            << "}"
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.cond = self.cond.specialize(generics, ns);
        self.block = self.block.specialize(generics, ns);
        self
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Transition {
    target: Token,
    cond: Option<Expr>,
}

impl Compile for Transition {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut t = Transition::default();
        t.target = token!(tokens.next().unwrap());
        if let Some(when) = tokens.next() {
            t.cond = Some(Expr::compile(when, ns)?);
        }
        Ok(t)
    }

    fn source(&self, process: &str) -> Lines {
        if let Some(cond) = &self.cond {
            Lines::new() << "if (__on_changed_event::changed(" << cond.source("") << ", __" << process << "_process)) {" << NewLine
                << "__" << process << "_process->change_state(__" << process
                << "_process_t::" << &self.target << ");" << NewLine
                << "break;" << NewLine
                << "}"
        } else {
            Lines::new() 
                << "__" << process << "_process->change_state(__" << process
                << "_process_t::" << &self.target << ");" << NewLine
        }
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.cond = self.cond.map(|e| e.specialize(generics, ns));
        self
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Default,  Serialize, Deserialize)]
pub struct State {
    pub(crate) ident: Token,
    transitions: Vec<Transition>,
    block: Block,
}

impl Compile for State {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut s = State::default();
        s.ident = token!(tokens.next().unwrap());
        while let Some(next) = tokens.next() {
            match next.as_rule() {
                Rule::goto => s.transitions.push(Transition::compile(next, ns)?),
                Rule::state_block => s.block = Block::compile(next, ns)?,
                _ => unreachable!("{:?}", next)
            }
        }
        Ok(s)
    }

    fn source(&self, process: &str) -> Lines {
        let mut lines = Lines::new()
            << "case __" << process << "_process_t::" << &self.ident << ": {" << NewLine
            << self.block.source("");
            for t in &self.transitions {
                lines += t.source(process) << NewLine;
            }
        lines << "break;" << NewLine
            << "}" << NewLine
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.transitions = self.transitions.iter().map(|t| t.clone().specialize(generics, ns)).collect();
        self
    }

    fn goto_def(&self) -> Option<Token> {
        Some(self.ident.clone())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Match {
    pat: Expr,
    arms: Vec<MatchArm>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchArm {
    case: Expr,
    block: Block,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Launch {
    process: Token,
    args: Vec<Expr>,
    init: Token,
    r#loop: Option<ForStmt>,
}

impl Compile for Launch {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut launch = Self::default();
        let mut tokens = token.into_inner();

        let launch_tokens = tokens.clone();

        while let Some(next) = tokens.next() {
            if next.as_rule() == Rule::for_stmt {
                launch.r#loop = Some(ForStmt::compile(next, ns)?);
            }
        }

        tokens = launch_tokens;
        launch.process = token!(tokens.next().unwrap());
        let mut next = tokens.next().unwrap();
        if next.as_rule() == Rule::pack {
            let mut exprs = next.into_inner();
            if let Some(lp) = &launch.r#loop {
                let var = Definition::Var(Var {
                    ident: lp.var.as_ref().unwrap().ident.clone(),
                    ty: if let Some(def) = lp.rang.def(ns) {
                        Type::Path(vec![], Some(Box::new(def.clone())))
                    } else {
                        Type::None
                    },
                    value: None,
                    is_directive: false,
                    val_is_signal: false,
                    name: None,
                    options: Vec::new()
                });
                ns.scoped(None, |scope| {
                    scope.borrow_mut().defs.push((lp.var.as_ref().unwrap().ident.clone(), var));
                    while let Some(expr) = exprs.next() {
                        launch.args.push(Expr::compile(expr, ns)?)
                    }
                    Ok(())
                })?
            } else {
                while let Some(expr) = exprs.next() {
                    launch.args.push(Expr::compile(expr, ns)?)
                }
            }
            next = tokens.next().unwrap();
        }
        launch.init = token!(next);

        Ok(launch)
    }

    fn source(&self, _ns: &str) -> Lines {
        let mut lines = Lines::new();
        if let Some(fr) = &self.r#loop {
            lines = fr.source("") << NewLine;
        }
        lines = lines << "__" << &self.process
            << "_process_t::launch(this, run_id, __scheduler, __"
            << &self.process << "_process_t::" << &self.init;
        for arg in &self.args {
            lines = lines << ", " << arg.source("");
        }
        lines << ");"
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.args = self.args.iter().map(|e| e.clone().specialize(generics, ns)).collect();
        self.r#loop = self.r#loop.map(|f| f.specialize(generics, ns));
        self
    }

    fn goto_def(&self) -> Option<Token> {
        Some(self.init.clone())
    }
}
