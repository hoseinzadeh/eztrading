
use super::{*, lines::Lines};
use crate::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Definition {
    Model(Struct),
    Module(Struct),
    Enum(Enum),
    GlobalVar(Var),     // let a: i32 = 12
    Var(Var),           // let a: i32 = 12
    Arg(Token, Type),   // fn func(a: int) { }
    Fn(Func),           // fn double<T>(T t) -> T { return t * 2; }
                        // event open(signal sig: bool, val: int) { return sig? val == 0 : false; }
    Cpp(Token),
}

impl Definition {
    pub fn as_struct(&self) -> Option<&Struct> {
        match self {
            Self::Model(s) | Self::Module(s) => Some(s),
            _ => None
        }
    }

    pub fn scope(&self, ns: &NameSpace) -> Option<Scope> {
        use Definition::*;
        match self {
            Model(s) | Module(s) => Some(s.scope.clone()),
            Enum(_) => todo!(),
            GlobalVar(var) | Var(var) => var.ty.def(ns).map(|s| s.scope(ns)).unwrap_or(None),
            Arg(_, t) => t.def(ns).map(|s| s.scope(ns)).unwrap_or(None),
            Fn(f) => if let Some(def) = f.inner().ret.def(ns) {
                def.scope(ns)
            } else {
                None
            },
            Cpp(_) => None,
        }
    }

    pub fn ty(&self) -> Option<&Type> {
        match self {
            Definition::GlobalVar(var) |
            Definition::Var(var) => Some(&var.ty),
            Definition::Arg(_, ty) => Some(&ty),
            Definition::Fn(f) => Some(&f.inner().ret),
            _ => None,
        }
    }

    pub fn is_signal(&self, ns: &NameSpace) -> bool {
        if let Definition::Var(v) = self {
            if v.ty.is_mutable() {
                return false;
            }
            if v.val_is_signal {
                return true;
            }
        }
        self.ty().map(|f| f.is_signal(None, ns)).unwrap_or(false)
    }
}

impl Compile for Definition {
    type Output = (Token, Self);

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<(Token, Self)> {
        match token.as_rule() {
            Rule::machine | Rule::module => {
                let s = Struct::compile(token, ns)?;
                Ok((s.as_struct().unwrap().ident.clone(), s))
            },
            Rule::event | Rule::func => {
                let f = Func::compile(token, ns)?;
                Ok((f.inner().ident.clone(), Self::Fn(f)))
            },
            Rule::cpp => Ok((Token::default(), Self::Cpp(token!(token)))),
            _ => unreachable!("{:?}", token)
        }
    }

    fn header(&self) -> Lines {
        match self {
            Definition::Model(m) |
            Definition::Module(m) => m.header() << NewLine,
            Definition::Enum(e) => e.header() << NewLine,
            Definition::Var(v) => v.header() << NewLine,
            Definition::Arg(arg, ty) => ty.header() << " " << arg << NewLine,
            Definition::Fn(f) => f.header() << NewLine,
            Definition::GlobalVar(v) => if v.is_directive {
                Lines::new()
            } else {
                Lines::new() << "extern " << v.header() << NewLine 
            },
            Definition::Cpp(_) => Lines::new() << NewLine,
        }
    }

    fn source(&self, ns: &str) -> Lines {
        match self {
            Definition::Model(m) |
            Definition::Module(m) => m.source(ns) << NewLine,
            Definition::Enum(e) => e.source(ns) << NewLine,
            Definition::Var(v) => v.source(ns) << NewLine,
            Definition::GlobalVar(v) => v.source(ns) << NewLine,
            Definition::Fn(f) => f.source(ns) << NewLine,
            Definition::Cpp(b) => Lines::new() << b << NewLine,
            _ => Lines::new()
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        use Definition::*;
        match self {
            Model(st) => Model(st.specialize(generics, ns)),
            Module(st) => Module(st.specialize(generics, ns)),
            Enum(e) => Enum(e.specialize(generics, ns)),
            GlobalVar(v) => GlobalVar(v.specialize(generics, ns)),
            Var(v) => Var(v.specialize(generics, ns)),
            Arg(a, t) => Arg(a, t.specialize(generics, ns)),
            Fn(f) => Fn(f.specialize(generics, ns)),
            Cpp(_) => self
        }
    }

    fn goto_def(&self) -> Option<Token> {
        match self {
            Definition::Model(s) | Definition::Module(s) => s.goto_def(),
            Definition::Enum(e) => e.goto_def(),
            Definition::GlobalVar(v) | Definition::Var(v) => v.goto_def(),
            Definition::Arg(t, _) => Some(t.clone()),
            Definition::Fn(f) => f.goto_def(),
            Definition::Cpp(_) => None,
        }
    }
}
