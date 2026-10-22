
use super::*;
use crate::token;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Func {
    Fn(Fn),
    Static(Fn),
    Const(Fn),
    Index(Fn),
    Cons(Fn),
    Event(Event),
    Process(ProcFn),
}

impl Func {
    pub fn inner(&self) -> &Fn {
        match self {
            Func::Fn(f)
            | Func::Index(f)
            | Func::Static(f)
            | Func::Const(f)
            | Func::Cons(f) => f,
            Func::Event(f) => &f.func,
            Func::Process(f) => &f.func
        }
    }
    pub fn inner_mut(&mut self) -> &mut Fn {
        match self {
            Func::Fn(f)
            | Func::Static(f)
            | Func::Index(f)
            | Func::Const(f)
            | Func::Cons(f) => f,
            Func::Event(f) => &mut f.func,
            Func::Process(f) => &mut f.func
        }
    }
}

impl Func {
    fn set_directive(mut self, is_directive: bool) -> Self {
        self.inner_mut().is_directive = is_directive;
        self
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Fn {
    pub(crate) ident: Token,
    pub(crate) args: Vec<(Token, Type)>,
    pub(crate) interval: Option<Interval>,
    pub(crate) ret: Type,
    pub(crate) block: Block,
    pub(crate) is_directive: bool,
    pub(crate) initializers: String,
}

fn compile_arg(mut tokens: Pairs<Rule>, ns: &NameSpace) -> Result<(Token, Type)> {
    let mut next = tokens.next().unwrap();
    Ok(if next.as_rule() == Rule::constant {
        next = tokens.next().unwrap();
        if next.as_rule() == Rule::refr {
            next = tokens.next().unwrap();
            (token!(next), Type::compile(tokens.next().unwrap(), ns)?.into_ref().into_const())
        } else if next.as_rule() == Rule::signal_ref {
            next = tokens.next().unwrap();
            (token!(next), Type::compile(tokens.next().unwrap(), ns)?.into_signal().into_ref().into_const())
        } else {
            (token!(next), Type::compile(tokens.next().unwrap(), ns)?)
        }
    } else if next.as_rule() == Rule::refr {
        next = tokens.next().unwrap();
        (token!(next), Type::compile(tokens.next().unwrap(), ns)?.into_ref())
    } else if next.as_rule() == Rule::signal_ref {
        next = tokens.next().unwrap();
        (token!(next), Type::compile(tokens.next().unwrap(), ns)?.into_signal().into_ref())
    } else {
        (token!(next), Type::compile(tokens.next().unwrap(), ns)?)
    })
}

fn compile_args(mut tokens: Pairs<Rule>, ns: &NameSpace) -> Result<Vec<(Token, Type)>> {
    let mut args = Vec::new();
    while let Some(token) = tokens.next() {
        args.push(compile_arg(token.into_inner(), ns)?);
    }
    Ok(args)
}

impl Compile for Func {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self> {
        let rule = token.as_rule();
        let mut f = Fn::default();

        if rule == Rule::init || rule == Rule::launcher {
            f.ident = ns.nid.borrow().current().clone();
            f.block = ns.scoped(None, |_| {
                Block::compile(token.into_inner().next().unwrap(), ns)
            })?;
            return Ok(Func::Cons(f));
        }
        let mut tokens = token.clone().into_inner();
        let mut next = tokens.next().unwrap();
        let is_directive = next.as_rule() == Rule::directive;
        if is_directive {
            next = tokens.next().unwrap();
        }
        let is_static = (next.as_rule() == Rule::modifier) && (next.as_str() == "static");
        let is_const = (next.as_rule() == Rule::modifier) && (next.as_str() == "const");

        Ok(match rule {
            Rule::func => if is_static {
                Func::Static(Fn::compile(token, ns)?)
            } else if is_const {
                Func::Const(Fn::compile(token, ns)?)
            } else {
                Func::Fn(Fn::compile(token, ns)?)
            },
            Rule::index => {
                Func::Index(Fn::compile(token, ns)?)
            },
            Rule::event => Func::Event(Event::compile(token, ns)?),
            Rule::process => Func::Process(ProcFn::compile(token, ns)?),
            _ => unreachable!("{:?}", rule)
        }.set_directive(is_directive))
    }

    fn header(&self) -> Lines {
        if self.inner().is_directive {
            Lines::new()
        } else {
            match self {
                Func::Cons(f)
                | Func::Index(f)
                | Func::Fn(f) => f.header() << ";",
                Func::Process(f) => f.header() << ";",
                Func::Event(f) => f.header() << ";",
                Func::Static(f) => Lines::new() << "static " << f.header() << ";",
                Func::Const(f) => f.header() << " const;",
            }
        }
    }

    fn source(&self, ns: &str) -> Lines {
        if self.inner().is_directive {
            Lines::new()
        } else {
            match self {
                Func::Cons(f)
                | Func::Fn(f)
                | Func::Index(f)
                | Func::Static(f) => f.source(ns),
                Func::Const(f) => f.source(&format!("{}|const", ns)),
                Func::Event(f) => f.source(ns),
                Func::Process(f) => f.source(ns),
            }
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        match self {
            Func::Fn(f) => Func::Fn(f.specialize(generics, ns)),
            Func::Static(f) => Func::Static(f.specialize(generics, ns)),
            Func::Index(f) => Func::Index(f.specialize(generics, ns)),
            Func::Const(f) => Func::Const(f.specialize(generics, ns)),
            Func::Cons(f) => Func::Cons(f.specialize(generics, ns)),
            Func::Event(f) => Func::Event(f.specialize(generics, ns)),
            Func::Process(f) => Func::Process(f.specialize(generics, ns)),
        }
    }

    fn goto_def(&self) -> Option<Token> {
        self.inner().goto_def()
    }
}

impl Compile for Fn {
    type Output = Self;
    
    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut next = tokens.next().unwrap();
        let mut f = Fn::default();
        if next.as_rule() == Rule::modifier || next.as_rule() == Rule::directive {
            next = tokens.next().unwrap();
        }
        if next.as_rule() == Rule::args {
            f.ident = Token::from("operator[]");
        } else {
            f.ident = token!(next);
            next = tokens.next().unwrap();
        }
        if next.as_rule() == Rule::args {
            f.args = compile_args(next.into_inner(), ns)?;
            next = tokens.next().unwrap();
        }
        if next.as_rule() == Rule::interval {
            f.interval = Some(Interval::compile(next, ns)?);
            next = tokens.next().unwrap();
        }
        if next.as_rule() == Rule::refr {
            f.ret = Type::compile(tokens.next().unwrap(), ns)?.into_ref();
            next = tokens.next().unwrap();
        } else if next.as_rule() == Rule::signal_ref {
            f.ret = Type::compile(tokens.next().unwrap(), ns)?.into_signal().into_ref().into_const();
            next = tokens.next().unwrap();
        } else if next.as_rule() == Rule::typ {
            f.ret = Type::compile(next, ns)?;
            next = tokens.next().unwrap();
        } else {
            f.ret = Type::Void;
        }
        f.block = ns.scoped(None, |scope| {
            for (arg, ty) in &f.args {
                scope.borrow_mut().defs.push((arg.clone(), Definition::Arg(arg.clone(), ty.clone())));
            }
            Block::compile(next, ns)
        })?;
        Ok(f)
    }

    fn header(&self) -> Lines {
        if self.is_directive { return Lines::new() }
        let mut lines = Lines::new();
        if !self.ret.is_none() {
            lines = lines << self.ret.header() << " ";
        }
        lines << &self.ident << "("
            << self.args.iter()
            .map(|(name, ty)| ty.header() << " " << name)
            .collect::<Lines>().join(", ") << ")"
    }

    fn source(&self, ns: &str) -> Lines {
        if self.is_directive { return Lines::new() }
        let mut lines = Lines::new();
        let parts: Vec<&str> = ns.split("|").collect();
        let mut renamed = self.clone();
        renamed.ident.ident = format!("{}{}", parts[0], self.ident.ident);
        lines = lines << renamed.header()
            << self.initializers.as_str()
            << if parts.len() == 2 {" const {"} else {" {"} << NewLine
            << self.block.source(ns) << NewLine
            << "}";
        lines
        
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.args = self.args.iter().map(|(a, t)| (a.clone(), t.clone().specialize(generics, ns).clone())).collect();
        self.block = self.block.specialize(generics, ns);
        self.ret = self.ret.specialize(generics, ns);
        self
    }

    fn goto_def(&self) -> Option<Token> {
        Some(self.ident.clone())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcFn {
    pub(crate) func: Fn
}

impl Compile for ProcFn {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self> {
        let mut me = Self {
            func: Fn::compile(token, ns)?
        };
        let name = me.func.ident.as_str();
        me.func.args.insert(0,
            (Token::from(format!("__{}_process", name).as_str()),
            Type::Path(vec![r#type::Path::Ident(
                Token::from(format!("__{}_process_t*", name).as_str()))], None)));
        Ok(me)
    }

    fn header(&self) -> Lines {
        self.func.header()
    }

    fn source(&self, ns: &str) -> Lines {
        let mut lines = Lines::new();
        let mut resubmit = false;
        lines = lines << "void " << ns << &self.func.ident << "("
            << self.func.args.iter()
            .map(|(name, ty)| ty.header() << " " << name)
            .collect::<Lines>().join(", ") << ") {" << NewLine
            << "auto *__current_process__ = __" << &self.func.ident << "_process;" << NewLine
            << self.func.block.source(ns) << NewLine;
        if let Some(int) = &self.func.interval {
            if !int.is_continual() {
                lines = lines << "__current_process__->set_next_cycle(system_clock::now());" << NewLine
                              << "__current_process__->reschedule();" << NewLine;
            } else {
                resubmit = true;
            }
        }

        
        lines = lines << "/* class __stack_t: public __stack_objects_t" << NewLine
            << "{" << NewLine
            << self.func.block.stack().into_iter().map(|l| l.header()).collect::<Lines>() << NewLine
            << "}" << NewLine
            << "*/" << NewLine;


        lines = lines << "char __cache_filepath[128];" << NewLine
            << "snprintf(__cache_filepath, 128, \"" << ns.replace("::", "_") << &self.func.ident << "_%u\", __run_id);" << NewLine
            << "__load_state(__cache_filepath);" << NewLine
            << "while (__current_process__->is_running()) {" << NewLine
            <<    "__current_process__->acquire();" << NewLine;

        if let Some(int) = &self.func.interval {
            if !int.is_continual() {
                lines = lines <<  "__current_process__->set_next_cycle(" << int.source("") << ");" << NewLine;
            }
        }
        lines = lines <<    "switch (__current_process__->state) {" << NewLine;
        for state in self.func.block.states() {
            lines += state.source(self.func.ident.as_str());
        }
        // todo!("interval?");
        if resubmit {
            lines << "}" << NewLine
                << "__current_process__->resubmit();" << NewLine
                << "}" << NewLine
                << "}"
        } else {
            lines << "}" << NewLine
                << "__save_state(__cache_filepath); // TODO: store all variables into a file" << NewLine
                << "__current_process__->notify();" << NewLine
                << "}" << NewLine
                << "}"
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        Self {
            func: self.func.specialize(generics, ns)
        }
    }

    fn goto_def(&self) -> Option<Token> {
        self.func.goto_def()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Interval {
    Continual,
    Millis(Expr, Option<Expr>),
    Second(Expr, Option<Expr>),
    Minute(Expr, Option<Expr>),
    Hour(Expr, Option<Expr>),
    Day(Expr, Time, Option<Expr>),
    Month(Expr, u32, Time, Option<Expr>),
    Year(Expr, u32, u32, Time, Option<Expr>),
    Custom(Expr, Option<Expr>)
}

impl Interval {
    pub fn is_continual(&self) -> bool {
        if let Self::Continual = self {
            true
        } else {
            false
        }
    }
}

impl Compile for Interval {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut next = tokens.next().unwrap();
        let mut rep: Expr = Expr::Num(Token::from("1"));
        let mut expr = None;
        let mut every_day = false;
        let mut every_month = false;
        if next.as_rule() == Rule::expr {
            rep = Expr::compile(next, ns)?;
            if let Some(n) = tokens.next() {
                next = n;
            } else {
                return Ok(if let Some(next) = tokens.next() {
                    Self::Custom(rep, Some(Expr::compile(next.clone(), ns)?))
                } else {
                    Self::Custom(rep, None)
                })
            }
        }
        let rule = next.as_rule();
        if rule == Rule::expr {
            expr = Some(Expr::compile(next.clone(), ns)?);
        }
        if rule == Rule::continual {
            return Ok(Self::Continual);
        }
        if rule == Rule::milli {
            return Ok(Self::Millis(rep, expr));
        }
        if rule == Rule::second {
            return Ok(Self::Second(rep, expr));
        }
        if rule == Rule::minute {
            return Ok(Self::Minute(rep, expr));
        }
        if rule == Rule::hour {
            return Ok(Self::Hour(rep, expr));
        }
        let mut next = None;
        if rule == Rule::day {
            every_day = true;
            next = tokens.next();
        }
        if rule == Rule::month {
            every_month = true;
            next = tokens.next();
        }
        if rule == Rule::year {
            next = tokens.next();
        }

        let mut day = 1;
        if let Some(nxt) = &next {
            if nxt.as_rule() == Rule::int {
                day = nxt.as_str().parse().unwrap();
                next = tokens.next();
            }
        }
        
        let mut mon = 1;
        if let Some(nxt) = &next {
            if nxt.as_rule() == Rule::date {
                let mut date = nxt.as_str().split("/");
                mon = date.next().unwrap().trim().parse().unwrap();
                day = date.next().unwrap().trim().parse().unwrap();
                next = tokens.next();
            }
        }

        let mut at = Time { hour: 9, min: 1, sec: 0 };
        if let Some(nxt) = &next {
            if nxt.as_rule() == Rule::at {
                at = Time::compile(nxt.clone().into_inner().next().unwrap(), ns)?;
                next = tokens.next();
            }
        }
        if let Some(nxt) = &next {
            if nxt.as_rule() == Rule::expr {
                expr = Some(Expr::compile(nxt.clone(), ns)?);
            }
        }
        Ok(if every_day {
            Self::Day(rep, at, expr)
        } else if every_month {
            Self::Month(rep, day, at, expr)
        } else {
            Self::Year(rep, mon, day, at, expr)
        })
    }

    /// Returns the from expr
    fn header(&self) -> Lines {
        match self {
            Interval::Continual => Lines::new(),
            Interval::Millis(_, e)
            | Interval::Second(_, e)
            | Interval::Minute(_, e)
            | Interval::Hour(_, e)
            | Interval::Day(_, _, e) 
            | Interval::Month(_, _, _, e)
            | Interval::Year(_, _, _, _, e)
            | Interval::Custom(_, e) => if let Some(expr) = e {
                expr.header()
            } else {
                Lines::new()
            },
        }
    }

    /// Returns the next cycle
    fn source(&self, _ns: &str) -> Lines {
        match self {
            Interval::Continual => Lines::new(),
            Interval::Millis(s, _) => Lines::new() << "system_clock::now() + duration::milliseconds(" << s.source("") << ")",
            Interval::Second(s, _) => Lines::new() << "system_clock::now() + duration::seconds(" << s.source("") << ")",
            Interval::Minute(m, _) => Lines::new() << "system_clock::now() + duration::minutes(" << m.source("") << ")",
            Interval::Hour(h, _) => Lines::new() << "system_clock::now() + duration::hours(" << h.source("") << ")",
            Interval::Day(d, t, _) => Lines::new() << "duration::days_at(" << d.source("") << ", " << t.source("") << ")",
            Interval::Month(m, d, t, _) => Lines::new() << "duration::months_at(" << m.source("") << ", " << d << ", " << t.source("") << ")",
            Interval::Year(y, m, d, t, _) => Lines::new() << "duration::years_at(" << y.source("") << ", " << m << ", " << d << ", " << t.source("") << ")",
            Interval::Custom(c, _) => Lines::new() << "system_clock::now() + " << c.source("")
        }
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Time {
    hour: u8,
    min: u8,
    sec: u8,
}

impl Compile for Time {
    type Output = Self;

    fn compile(token: Pair<Rule>, _ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner(); // clock
        let mut clock = tokens.next().unwrap().as_str().split(":"); // clock_num
        let hour = clock.next().unwrap();
        let mut hour: u8 = hour.trim().parse().unwrap();
        let min: u8 = clock.next().unwrap().trim().parse().unwrap();
        let mut sec = 0;
        if let Some(s) = clock.next() {
            sec = s.trim().parse().unwrap();
        }
        if let Some(pm) = tokens.next() {
            if pm.as_rule() == Rule::pm {
                if hour < 12 { hour += 12; }
            }
        }
        Ok(Time { hour, min, sec })
    }

    fn source(&self, _ns: &str) -> Lines {
        Lines::new() << format!("{}, {}, {}", self.hour, self.min, self.sec)
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}


#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    func: Fn
}

impl Compile for Event {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self> {
        let mut me = Self {
            func: Fn::compile(token, ns)?
        };
        me.func.args.push((Token::from("__current_process__"), Type::Prim(Token::from("Worker*"))));
        me.func.ret = Type::Prim(Token::from("bool"));
        Ok(me)
    }

    fn header(&self) -> Lines {
        self.func.header()
    }

    fn source(&self, ns: &str) -> Lines {
        self.func.source(ns)
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        Self {
            func: self.func.specialize(generics, ns)
        }
    }

    fn goto_def(&self) -> Option<Token> {
        self.func.goto_def()
    }
}