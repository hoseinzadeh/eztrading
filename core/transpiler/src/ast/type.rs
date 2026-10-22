use super::*;
use crate::*;
use super::lines::Lines;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Type {
    Ref(Box<Type>),
    Static(Box<Type>),
    Const(Box<Type>),
    Signal(Box<Type>),
    Param(Box<Type>),
    Pub(Box<Type>),
    Array(Box<Type>, Box<Expr>),
    Path(Vec<Path>, Option<Box<Definition>>),
    Prim(Token),
    Scheduler,
    Mutable,
    Void,
    None
}

impl Type {
    pub fn or(&self, def: &str) -> Self {
        match self {
            Type::Ref(t) => Type::Ref(Box::new(t.or(def))),
            Type::Static(t) => Type::Static(Box::new(t.or(def))),
            Type::Const(t) => Type::Const(Box::new(t.or(def))),
            Type::Signal(t) => Type::Signal(Box::new(t.or(def))),
            Type::Param(t) => Type::Param(Box::new(t.or(def))),
            Type::Pub(t) => Type::Pub(Box::new(t.or(def))),
            Type::Array(t, e) => Type::Array(Box::new(t.or(def)), e.clone()),
            Type::Path(_, _) => self.clone(),
            Type::Prim(_) => self.clone(),
            Type::Void | Type::Mutable | Type::Scheduler => self.clone(),
            Type::None => Self::Prim(Token::from(def)),
        }
    }

    pub fn inner(&self) -> Type {
        match &self {
            Type::Ref(t) => t.inner(),
            Type::Static(t) => t.inner(),
            Type::Const(t) => t.inner(),
            Type::Signal(t) => t.inner(),
            Type::Param(t) => t.inner(),
            Type::Pub(t) => t.inner(),
            Type::Array(t, _) => t.inner(),
            Type::Path(_, _)
            | Type::Prim(_)
            | Type::Void
            | Type::Scheduler
            | Type::Mutable
            | Type::None => self.clone(),
        }
    }

    pub fn is_pub(&self) -> bool {
        match self {
            Type::Pub(_) => true,
            Type::Ref(t)
            | Type::Array(t, _)
            | Type::Const(t)
            | Type::Param(t)
            | Type::Signal(t) => t.is_pub(),
            _ => false
        }
    }

    pub fn is_param(&self) -> bool {
        match self {
            Type::Param(_) => true,
            Type::Ref(t)
            | Type::Const(t)
            | Type::Array(t, _)
            | Type::Pub(t)
            | Type::Signal(t) => t.is_param(),
            _ => false
        }
    }

    pub fn is_static(&self) -> bool {
        match self {
            Type::Static(_) => true,
            Type::Ref(t)
            | Type::Param(t)
            | Type::Array(t, _)
            | Type::Const(t)
            | Type::Pub(t)
            | Type::Signal(t) => t.is_static(),
            _ => false
        }
    }

    pub fn is_ref(&self) -> bool {
        match self {
            Type::Ref(_) => true,
            Type::Static(t)
            | Type::Param(t)
            | Type::Array(t, _)
            | Type::Const(t)
            | Type::Pub(t)
            | Type::Signal(t) => t.is_ref(),
            _ => false
        }
    }

    // pub fn is_array(&self) -> bool {
    //     match self {
    //         Type::Array(_, _) => true,
    //         Type::Static(t)
    //         | Type::Param(t)
    //         | Type::Ref(t)
    //         | Type::Const(t)
    //         | Type::Pub(t)
    //         | Type::Signal(t) => t.is_array(),
    //         _ => false
    //     }
    // }

    pub fn as_array(&self) -> Option<&Self> {
        match self {
            Type::Array(_, _) => Some(self),
            Type::Static(t)
            | Type::Param(t)
            | Type::Ref(t)
            | Type::Const(t)
            | Type::Pub(t)
            | Type::Signal(t) => t.as_array(),
            _ => None
        }
    }

    pub fn is_none(&self) -> bool {
        if let Self::None = self {
            true
        } else {
            false
        }
    }

    // pub fn is_scheduler(&self) -> bool {
    //     if let Self::Scheduler = self {
    //         true
    //     } else {
    //         false
    //     }
    // }

    pub fn is_mutable(&self) -> bool {
        if let Self::Mutable = self {
            true
        } else {
            false
        }
    }

    pub fn wrapped_in_signal(&self) -> bool {
        match self {
            Type::Signal(_) => true,
            Type::Static(t)
            | Type::Param(t)
            | Type::Const(t)
            | Type::Ref(t) => t.wrapped_in_signal(),
            _ => false
        }
    }

    pub fn get_path(&self) -> Option<Type> {
        match self {
            Type::Path(_, _) => Some(self.clone()),
            Type::Static(t)
            | Type::Param(t)
            | Type::Signal(t)
            | Type::Const(t)
            | Type::Pub(t)
            | Type::Ref(t) => t.get_path(),
            _ => None
        }
    }

    fn is_signal_type(&self) -> bool {
        match self {
            Type::Signal(_) => true,
            Type::Static(t)
            | Type::Param(t)
            | Type::Const(t)
            | Type::Pub(t)
            | Type::Ref(t) => t.is_signal_type(),
            _ => false
        }
    }

    pub fn is_signal(&self, default: Option<Expr>, ns: &NameSpace) -> bool {
        match self {
            Type::Signal(_) => true,
            Type::Static(t)
            | Type::Param(t)
            | Type::Const(t)
            | Type::Pub(t)
            | Type::Ref(t) => t.is_signal(default, ns),
            Type::Path(_, d) => {
                if let Some(def) = d {
                    return match def.as_ref() {
                        Definition::GlobalVar(v) |
                        Definition::Var(v) => v.ty.is_signal(v.value.clone(), ns),
                        Definition::Arg(_, ty) => ty.is_signal(None, ns),
                        Definition::Fn(f) => f.inner().ret.is_signal(None, ns),
                        _ => false,
                    };
                }
                false
            }
            Type::None => {
                if let Some(e) = default {
                    if let Some(def) = e.def(ns) {
                        return match def {
                            Definition::GlobalVar(v) |
                            Definition::Var(v) => v.ty.is_signal(v.value.clone(), ns),
                            Definition::Arg(_, ty) => ty.is_signal(None, ns),
                            Definition::Fn(f) => f.inner().ret.is_signal(None, ns),
                            _ => false,
                        };
                    }
                }
                false
            }
            _ => false
        }
    }

    pub fn is_const(&self) -> bool {
        match self {
            Type::Const(_) => true,
            Type::Static(t)
            | Type::Param(t)
            | Type::Signal(t)
            | Type::Pub(t)
            | Type::Ref(t) => t.is_const(),
            _ => false
        }
    }

    pub fn into_ref(self) -> Self {
        Type::Ref(Box::new(self))
    }

    pub fn into_static(self) -> Self {
        if self.is_static() { self } else { Type::Static(Box::new(self)) }
    }

    pub fn into_pub(self) -> Self {
        if self.is_pub() { self } else { Type::Pub(Box::new(self)) }
    }

    pub fn into_param(self) -> Self {
        if self.is_param() { self } else { Type::Param(Box::new(self)) }
    }

    pub fn into_const(self) -> Self {
        if self.is_const() { self } else { Type::Const(Box::new(self)) }
    }

    pub fn into_signal(self) -> Self {
        if self.wrapped_in_signal() { self } else { Type::Signal(Box::new(self)) }
    }

    pub fn drop_static(self) -> Self {
        match self {
            Type::Static(t) => *t,
            Type::Const(t) => Type::Const(Box::new(t.drop_static())),
            Type::Signal(t) => Type::Signal(Box::new(t.drop_static())),
            Type::Ref(t) => Type::Ref(Box::new(t.drop_static())),
            Type::Pub(t) => Type::Pub(Box::new(t.drop_static())),
            Type::Param(_) => unreachable!("{:?}", self),
            _ => self,
        }
    }

    pub fn def(&self, ns: &NameSpace) -> Option<Definition> {
        match self {
            Type::Ref(t)
            | Type::Static(t)
            | Type::Signal(t)
            | Type::Param(t)
            | Type::Array(t, _)
            | Type::Pub(t)
            | Type::Const(t) => t.def(ns),
            Type::Path(p, _) => Path::final_def(p, ns).map(|p| p.clone()),
            Type::Prim(_)
            | Type::Void
            | Type::Mutable
            | Type::Scheduler
            | Type::None => None,
        }
    }
}

impl Default for Type {
    fn default() -> Self {
        Type::None
    }
}

impl Compile for Type {
    type Output = Self;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self> {
        let mut tokens = token.into_inner();
        let mut token = tokens.next().unwrap();
        let mut is_signal = false;
        if token.as_rule() == Rule::signal_kw {
            token = tokens.next().unwrap();
            is_signal = true;
        }

        fn to_signal(t: Type, is_signal: bool, ns: &NameSpace) -> Type {
            if is_signal && !t.is_signal(None, ns) {
                Type::Signal(Box::new(t))
            } else {
                t
            }
        }

        let res = match token.as_rule() {
            Rule::primary_types => to_signal(Type::Prim(token!(token)), is_signal, ns),
            Rule::typ_path => {
                let p = Path::compile(token, ns)?;
                let res = Path::final_def(&p, ns);
                to_signal(Type::Path(p, res.map(|p| Box::new(p.clone()))), is_signal, ns)
            },
            _ => unreachable!("{:?}", token)
        };

        Ok(if let Some(arr) = tokens.next() {
            Self::Array(Box::new(res), Box::new(Expr::compile(arr, ns)?))
        } else {
            res
        })
    }

    fn header(&self) -> Lines {
        let mut lines = Lines::new();
        match self {
            Type::Path(v, _) => { for p in v { lines.write_lines(p.header());} lines},
            Type::Prim(p) => lines << p,
            Type::Void => lines << "void",
            Type::Mutable  => lines << "auto&",
            Type::Scheduler => lines << "Scheduler*",
            Type::None  => lines,
            Type::Pub(t) => t.header(),
            Type::Param(t) => t.header(),
            Type::Ref(t)
            | Type::Static(t)
            | Type::Array(t, _)
            | Type::Const(t)
            | Type::Signal(t) => {
                let ty = t.inner();
                if self.is_static() {
                    lines = lines << "static ";
                }
                if self.is_const() {
                    lines = lines << "const ";
                }
                let mut suffix = Lines::new();
                if self.is_ref() {
                    suffix = suffix << "&";
                }
                if self.is_signal_type() {
                    lines << "Signal<" << ty.header() << ">" << suffix
                } else {
                    lines << ty.header() << suffix
                }
            }

            // Type::Ref(t) => t.header() << "&",
            // Type::Static(t) => lines << "static " << t.header(),
            // Type::Const(t) => lines << "const " << t.header(),
            // Type::Signal(t) => lines << "Signal<" << t.header() << ">",
            // Type::Param(t) => t.header(),
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        match self {
            Type::Ref(t) => Type::Ref(Box::new(t.specialize(generics, ns))),
            Type::Static(t) => Type::Static(Box::new(t.specialize(generics, ns))),
            Type::Const(t) => Type::Const(Box::new(t.specialize(generics, ns))),
            Type::Signal(t) => Type::Signal(Box::new(t.specialize(generics, ns))),
            Type::Param(t) => Type::Param(Box::new(t.specialize(generics, ns))),
            Type::Pub(t) => Type::Pub(Box::new(t.specialize(generics, ns))),
            Type::Array(t, e) => Type::Array(Box::new(t.specialize(generics, ns)), Box::new(e.specialize(generics, ns))),
            Type::Path(v, d) => 
            {
                if v.len() == 1 {
                    if let Path::Ident(id) = &v[0] {
                        if let Some(ty) = generics.get(&id) {
                            return ty.clone();
                        }
                    }
                }
                Type::Path(v.iter().map(|t| t.clone().specialize(generics, ns)).collect(), d.map(|d| Box::new(d.specialize(generics, ns))))
            }
            Type::Prim(t) => if let Some(ty) = generics.get(&t) { ty.clone() } else { Type::Prim(t) }
            Type::Void
            | Type::Mutable
            | Type::Scheduler
            | Type::None => self,
        }
    }

    fn goto_def(&self) -> Option<Token> {
        match self {
            Type::Ref(t)
            | Type::Static(t)
            | Type::Const(t)
            | Type::Signal(t)
            | Type::Param(t)
            | Type::Pub(t)
            | Type::Array(t, _) => t.goto_def(),
            Type::Path(_, Some(def)) => def.ty().map(|t| t.goto_def()).unwrap_or(None),
            Type::Prim(t) => Some(t.clone()),
            _ => None
        }
    }

}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Path {
    Ident(Token),           // a::b
    Spec(Token, Vec<Type>), // a::b<c>
}

impl Path {
    pub fn def<'a>(&self, ns: &'a NameSpace) -> Option<Definition> {
        match self {
            Path::Ident(id) => ns.find_def(id, |def| Some(def.clone())),
            Path::Spec(id, tmp) => {
                if let Some(def) = ns.find_def(id, |def| Some(def.clone()) ) {
                    let mut generics = HashMap::new();
                    if let Definition::Model(st) | Definition::Module(st) = &def {
                        assert!(st.temp.len() == tmp.len(), "Number of generic types unmatched");
                        for i in 0..tmp.len() {
                            generics.insert(st.temp[i].clone(), tmp[i].clone());
                        }
                        Some(def.specialize(&mut generics, ns))
                    } else {
                        Some(def)
                    }
                } else {
                    None
                }
            }
        }
    }

    pub fn final_def<'a>(p: &Vec<Self>, ns: &'a NameSpace) -> Option<Definition> {
        let mut pushed = 0;
        for i in 0..p.len()-1 {
            if let Some(def) = p[i].def(ns) {
                if ns.new_unnamed_scope(&def) {
                    pushed+=1;
                }
            } else {
                todo!("Scope: {:?}", p[i])
            }
        }
        let res = p.last().unwrap().def(ns);
        for _ in 0..pushed {
            ns.drop_unnamed_scope();
        }
        res
    }
}

impl Compile for Path {
    type Output = Vec<Self>;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Vec<Self>> {
        let mut path = vec!();
        let mut last = None;
        let mut tokens = token.into_inner();

        while let Some(token) = tokens.next() {
            match token.as_rule() {
                Rule::ident => {
                    if let Some(last) = last {
                        path.push(Path::Ident(token!(last)))
                    }
                    last = Some(token);
                },
                Rule::temp => {
                    let mut temp = vec![];
                    let mut types = token.into_inner();
                    while let Some(ty) = types.next() {
                        temp.push(Type::compile(ty, ns)?)
                    }
                    if let Some(last) = last {
                        path.push(Path::Spec(token!(last), temp))
                    }
                    last = None;
                }
                _ => unreachable!("{:?}", token)
            }
        }
        if let Some(last) = last {
            path.push(Path::Ident(token!(last)))
        }
        Ok(path)
    }

    fn header(&self) -> Lines {
        let lines = Lines::new();
        match self {
            Path::Ident(t) => lines << t,
            Path::Spec(t, v) => {
                lines << t << "<" << v.iter().map(|ty| ty.header()).collect::<Lines>().join(", ") << ">"
            },
        }
    }

    fn specialize(self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        match self {
            Path::Ident(id) => Path::Ident(id),
            Path::Spec(id, tmps) => Path::Spec(id, tmps.iter().map(|t| t.clone().specialize(generics, ns)).collect()),
        }
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}
