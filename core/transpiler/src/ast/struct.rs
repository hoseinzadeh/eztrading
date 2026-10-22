use super::{
    lines::{Lines, NewLine},
    *,
};
use crate::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Struct {
    pub(crate) ident: Token,
    pub(crate) bases: Vec<(Type, Definition)>,
    pub(crate) temp: Vec<Token>,
    pub(crate) scope: Scope,
    pub(crate) is_directive: bool,
    pub(crate) is_model: bool,
}

impl Struct {
    fn initializer(
        &self,
        add_params_var: bool,
        extract_params: bool,
        including_bases: bool,
        including_vars: bool,
        ns: &NameSpace,
    ) -> Vec<Statement> {
        let mut res = vec![];
        if add_params_var {
            res.push(Statement::Expr(Expr::Infix(
                Box::new(Expr::Ident(Token::from("auto params"), None)),
                Box::new(Expr::Opr(Token::from("="))),
                Box::new(Expr::Ident(
                    Token::from("__contents::params_from_json(params_json)"),
                    None,
                )),
            )));
        }
        for (_, def) in &self.scope.defs {
            if let Definition::Var(v) = def {
                if v.ty.is_param() && extract_params {
                    let val = v.value.clone().unwrap_or(Expr::Ident(
                        Token::from(format!("{}()", v.ty.header().print("").trim()).as_str()),
                        None,
                    ));
                    res.push(Statement::Expr(Expr::Infix(
                        Box::new(Expr::Ident(v.ident.clone(), None)),
                        Box::new(Expr::Opr(Token::from("="))),
                        Box::new(Expr::Infix(
                            Box::new(Expr::Ident(
                                Token::from(
                                    format!("params.contains(\"{}\")", v.ident.as_str()).as_str(),
                                ),
                                None,
                            )),
                            Box::new(Expr::Ident(
                                Token::from(
                                    format!(
                                        "__contents::from_any<{}>(params[\"{}\"])",
                                        v.ty.header().print("").trim(),
                                        v.ident.as_str()
                                    )
                                    .as_str(),
                                ),
                                None,
                            )),
                            Box::new(val),
                        )),
                    )))
                } else if let Some(val) = &v.value {
                    if including_vars || !including_bases {
                        res.push(Statement::Expr(Expr::Infix(
                            Box::new(Expr::Ident(v.ident.clone(), None)),
                            Box::new(Expr::Opr(Token::from("="))),
                            Box::new(val.clone()),
                        )))
                    }
                }
            }
        }
        if including_bases {
            for (_, d) in &self.bases {
                if let Definition::Module(m) = d {
                    let mut inherited =
                        m.initializer(false, extract_params, true, including_vars, ns);
                    res.append(&mut inherited);
                }
            }
        }
        res
    }

    fn extract_params(&self) -> Vec<&Var> {
        let mut res = vec![];
        for (_, def) in &self.scope.defs {
            if let Definition::Var(v) = def {
                if v.ty.is_param() {
                    res.push(v)
                }
            }
        }
        for (_, d) in &self.bases {
            if let Definition::Module(m) = d {
                res.append(&mut m.extract_params());
            }
        }
        res
    }
}

impl Compile for Struct {
    type Output = Definition;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Definition> {
        let is_model = token.as_rule() == Rule::machine; // otherwise, module
        let mut tokens = token.into_inner();
        let mut s = Struct::default();
        s.is_model = is_model;
        s.scope.nid = ns.nid.borrow().clone();
        let next = tokens.next().unwrap();
        if next.as_rule() == Rule::directive {
            s.is_directive = true;
            s.ident = token!(tokens.next().unwrap());
        } else {
            s.ident = token!(next);
        }
        let mut next = tokens.next().unwrap();
        while next.as_rule() == Rule::ident {
            s.temp.push(token!(next));
            next = tokens.next().unwrap();
        }
        while next.as_rule() == Rule::typ {
            let ty = Type::compile(next, ns)?;
            s.bases.push((
                ty.clone(),
                ty.def(ns).expect(
                    format!("No definition found for '{}'",ty.header().print("").trim()).as_str(),
                ),
            ));
            next = tokens.next().unwrap();
        }
        let bases: Vec<(Type, Definition)> = s.bases.clone();
        let mut initializer = s.initializer(is_model, is_model, true, false, ns);
        ns.scoped(Some(s.ident.clone()), |scope| {
            for (ty, base_ty) in &bases {
                if let Definition::Module(base) = base_ty {
                    let mut sc = base.scope.defs.clone();
                    scope.borrow_mut().defs.append(&mut sc);
                } else {
                    panic!("Cannot inherit from {}", ty.header().print("").trim())
                }
            }
            scope
                .borrow_mut()
                .defs
                .push((s.ident.clone(), Definition::Module(s.clone())));
            let mut tokens = next.into_inner();
            let mut process_tokens = vec![];
            while let Some(token) = tokens.next() {
                match token.as_rule() {
                    Rule::param
                    | Rule::signal
                    | Rule::typed_var
                    | Rule::typed_const
                    | Rule::pub_signal
                    | Rule::pub_var
                    | Rule::pub_const
                    | Rule::typed_static
                    | Rule::static_signal => {
                        let vars = Var::compile(token, ns)?;
                        for var in vars {
                            let id = var.ident.clone();
                            scope.borrow_mut().defs.pop();
                            scope
                                .borrow_mut()
                                .defs
                                .push((id.clone(), Definition::Var(var.clone())));
                            s.scope.defs.push((id, Definition::Var(var)));
                            scope
                                .borrow_mut()
                                .defs
                                .push((s.ident.clone(), Definition::Module(s.clone())));
                        }
                    }
                    Rule::process => process_tokens.push(token.clone()),
                    Rule::init | Rule::func | Rule::event | Rule::launcher | Rule::index => {
                        let is_init = token.as_rule() == Rule::launcher;
                        let mut f = Func::compile(token, ns)?;
                        let id = if f.inner().ident.as_str().is_empty() {
                            s.ident.clone()
                        } else {
                            f.inner().ident.clone()
                        };
                        if is_init && is_model {
                            let func = f.inner_mut();
                            func.args.insert(
                                0,
                                (Token::from("params_json"), Type::Prim(Token::from("char*"))),
                            );
                            func.args
                                .insert(0, (Token::from("scheduler"), Type::Scheduler));
                            func.args.insert(
                                0,
                                (Token::from("run_id"), Type::Prim(Token::from("uint32_t"))),
                            );
                        }
                        scope.borrow_mut().defs.pop();
                        scope
                            .borrow_mut()
                            .defs
                            .push((id.clone(), Definition::Fn(f.clone())));
                        s.scope.defs.push((id, Definition::Fn(f)));
                        scope
                            .borrow_mut()
                            .defs
                            .push((s.ident.clone(), Definition::Module(s.clone())));
                    }
                    _ => unreachable!("{:?}", token),
                }
            }

            for process in process_tokens {
                let f = Func::compile(process, ns)?;
                let id = if f.inner().ident.as_str().is_empty() {
                    s.ident.clone()
                } else {
                    f.inner().ident.clone()
                };
                scope.borrow_mut().defs.pop();
                scope
                    .borrow_mut()
                    .defs
                    .push((id.clone(), Definition::Fn(f.clone())));
                s.scope.defs.push((id, Definition::Fn(f)));
                scope
                    .borrow_mut()
                    .defs
                    .push((s.ident.clone(), Definition::Module(s.clone())));
            }

            fn gen_initializers(s: &mut Struct, bases: &Vec<(Type, Definition)>) -> String {
                let mut init = String::new();
                let mut first = true;
                if !s.bases.is_empty() {
                    for (ty, _) in bases {
                        if first {
                            init += ": "
                        } else {
                            init += ", "
                        }
                        first = false;
                        init += format!("{}()", ty.header().print("").trim().to_owned()).as_str()
                    }
                }
                if s.is_model {
                    if first {
                        init += ": "
                    } else {
                        init += ", "
                    }
                    init += "__run_id(run_id), __scheduler(scheduler)";
                }
                init
            }

            initializer.append(&mut s.initializer(false, is_model, false, true, ns));
            let mut has_init = false;
            let initalizers = gen_initializers(&mut s, &bases);
            for (_, d) in &mut s.scope.defs {
                if let Definition::Fn(Func::Cons(init)) = d {
                    init.initializers = initalizers.clone();
                    has_init = true;
                    if is_model {
                        if let Block::Block(block) = &mut init.block {
                            block.splice(0..0, initializer.iter().cloned());
                        }
                    }
                    break;
                }
            }

            if !has_init {
                let mut init = Fn::default();
                init.ident = s.ident.clone();
                init.initializers = initalizers;
                if is_model {
                    init.args
                        .push((Token::from("run_id"), Type::Prim(Token::from("uint32_t"))));
                    init.args.push((Token::from("scheduler"), Type::Scheduler));
                    init.args
                        .push((Token::from("params_json"), Type::Prim(Token::from("char*"))));
                }
                init.block = Block::Block(initializer);
                s.scope
                    .defs
                    .push((ns.nid.borrow().current().clone(), Definition::Fn(Func::Cons(init))));
            }

            if is_model {
                let mut state_fn = Fn::default();
                state_fn.ident = Token::from("__save_state");
                state_fn.args.push((Token::from("filepath"), Type::Prim(Token::from("const char*"))));
                state_fn.ret = Type::Void;
                s.scope.defs.push((ns.nid.borrow().current().clone(), Definition::Fn(Func::Fn(state_fn.clone()))));

                state_fn.ident = Token::from("__load_state");
                s.scope.defs.push((ns.nid.borrow().current().clone(), Definition::Fn(Func::Fn(state_fn))));
                // TODO: Add the code block for both functions
            }

            Ok(if is_model {
                Definition::Model(s)
            } else {
                Definition::Module(s)
            })
        })
    }

    fn header(&self) -> Lines {
        if self.is_directive {
            return Lines::new();
        }
        let mut lines = Lines::new();

        if !self.temp.is_empty() {
            lines = lines << "template <class " << self.temp.join(", class") << ">" << NewLine;
        }

        lines = lines << "class " << &self.ident;
        if self.is_model {
            lines = lines << ": public sml::machine";
        }

        if !self.bases.is_empty() {
            lines = lines << if self.is_model {
                    ", public "
                } else {
                    ": public "
                } << self
                    .bases
                    .iter()
                    .map(|(ty, _)| ty.header())
                    .collect::<Lines>()
                    .join(", public ");
        }
        lines = lines << " {" << NewLine;

        lines = lines << "protected:" << NewLine;

        // Non-static variables
        for (_, v) in &self.scope.defs {
            if let Definition::Var(v) = v {
                if !v.ty.is_static() && !v.ty.is_pub() {
                    lines = lines << v.header() << NewLine;
                }
            }
        }
        if self.is_model {
            lines = lines << "uint32_t __run_id;" << NewLine
                    << "Scheduler *__scheduler;" << NewLine
                    << "std::vector<Worker*> workers;" << NewLine;
        }

        for f in &self.scope.defs {
            if let (fname, Definition::Fn(Func::Process(func))) = f {
                let func = &func.func;
                lines = lines << "class __" << fname << "_process_t: public Worker {" << NewLine
                        << "public:" << NewLine
                        << "__" << fname << "_process_t(uint32_t run_id) : Worker(run_id) {}" << NewLine
                        << "enum state_t {" << NewLine;
                for s in func.block.states() {
                    lines = lines << &s.ident << "," << NewLine;
                }
                lines = lines << "} state;" << NewLine
                        << "void change_state(state_t state) {" << NewLine
                        << "this->state = state;" << NewLine
                        << "}" << NewLine
                        << "static void launch(" << &self.ident << " *machine, uint32_t run_id, Scheduler *scheduler, state_t state";
                for (arg, ty) in &func.args[1..] {
                    lines = lines << ", " << ty.header() << " " << arg;
                }
                lines = lines << ") {" << NewLine
                        << "__" << fname << "_process_t *__" << fname << "_process = new __" << fname << "_process_t(run_id);" << NewLine
                        << "__" << fname << "_process->setup(scheduler);" << NewLine
                        << "__" << fname << "_process->running.store(true);" << NewLine
                        << "__" << fname << "_process->state = state;" << NewLine
                        << "__" << fname << "_process->thread = new std::thread(__" << fname << "_process_t::work, machine";
                for (arg, ty) in &func.args {
                    if ty.is_ref() {
                        lines = lines << ", std::cref(" << arg << ")";
                    } else {
                        lines = lines << ", " << arg;
                    }
                }
                lines = lines << ");" << NewLine
                        << "machine->workers.push_back(__" << fname << "_process);" << NewLine
                        << "}" << NewLine
                        << "static void work(" << &self.ident << " *process";
                for (arg, ty) in &func.args {
                    lines = lines << ", " << ty.header() << " " << arg;
                }
                lines = lines << ") {" << NewLine
                        << "process->" << fname << "(" << func
                        .args
                        .iter()
                        .map(|(a, _)| a.clone())
                        .collect::<Vec<Token>>()
                        .join(", ") << ");" << NewLine
                        << "}" << NewLine
                        << "};" << NewLine
            }
        }

        lines = lines << "public:" << NewLine;
        if self.is_model {
            lines = lines << "static __contents::machine __get_contents() {" << NewLine
                    << "__contents::machine machine(\"" << self.scope.nid.as_str() << "::" << &self.ident << "\");" << NewLine
                    << "__contents::param param(\"\");" << NewLine;
            let params = self.extract_params();
            for v in params {
                let name = if let Some(n) = &v.name {
                    n.clone()
                } else {
                    Token::from(format!("\"{}\"", v.ident.as_str()).as_str())
                };
                lines = lines << "param = __contents::param(\"" << v.ident.as_str() << "\", " << &name << ");" << NewLine
                    << "param.type = \"" << v.ty.header() << "\";" << NewLine
                    << "param.value = __contents::mk_any<" << v.ty.header() << ">(" << if let Some(expr) = v.value.as_ref() {
                        expr.source("")
                    } else {
                        v.ty.header() << "()"
                    } << ");" << NewLine;
                for opt in &v.options {
                    let name = if let Some(n) = &opt.1 {
                        n.clone()
                    } else {
                        Token::from(format!("\"{}\"", opt.0.source("").print("").trim()).as_str())
                    };
                    lines = lines << "param.option_strings.push_back(" << &name << ");" << NewLine
                            << "param.option_values.push_back(__contents::mk_any<" << v.ty.header() << ">(" << opt.0.source("") << "));" << NewLine;
                }
                lines = lines << "machine.params.push_back(param);" << NewLine;
            }
            lines = lines << "return machine;" << NewLine
                    << "}" << NewLine
                    << "void die() {" << NewLine
                    << "for (Worker *worker: workers) {" << NewLine
                    << "worker->die();" << NewLine
                    << "}" << NewLine
                    << "}" << NewLine;
        }

        // Static variables
        for (_, v) in &self.scope.defs {
            if let Definition::Var(v) = v {
                if v.ty.is_static() || v.ty.is_pub() {
                    lines = lines << v.header() << NewLine;
                }
            }
        }

        for f in &self.scope.defs {
            if let (_, Definition::Fn(f)) = f {
                lines = lines << f.header() << NewLine;
            }
        }

        lines << "};" << NewLine
    }

    fn source(&self, _: &str) -> Lines {
        if self.is_directive {
            return Lines::new();
        }
        let mut lines = Lines::new();
        let mut namespace = self.ident.as_str().to_owned();
        let mut temp_types = Lines::new();
        if !self.temp.is_empty() {
            temp_types =
                temp_types << "template <class " << self.temp.join(", class") << ">" << NewLine;
            namespace += &format!(
                "<{}>",
                self.temp
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<&str>>()
                    .join(", ")
            );
        }
        namespace += "::";
        let ns = &namespace;
        for (_, v) in &self.scope.defs {
            if let Definition::Var(v) = v {
                if v.ty.is_static() {
                    lines = lines << temp_types.clone() << v.source(&ns) << NewLine;
                }
            }
        }

        lines = lines << NewLine;
        for f in &self.scope.defs {
            if let (_, Definition::Fn(f)) = f {
                lines = lines << temp_types.clone() << f.source(&ns) << NewLine;
            }
        }

        lines
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.bases = self
            .bases
            .iter()
            .map(|(t, d)| {
                (
                    t.clone().specialize(generics, ns),
                    d.clone().specialize(generics, ns),
                )
            })
            .collect();
        self.scope.defs = self
            .scope
            .defs
            .iter()
            .map(|(t, d)| (t.clone(), d.clone().specialize(generics, ns)))
            .collect();
        self
    }

    fn goto_def(&self) -> Option<Token> {
        Some(self.ident.clone())
    }
}
