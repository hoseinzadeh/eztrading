
use super::*;
use crate::*;
use super::lines::Lines;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Var {
    pub(crate) ident: Token,
    pub(crate) ty: Type,
    pub(crate) value: Option<Expr>,
    pub(crate) is_directive: bool,
    pub(crate) val_is_signal: bool,
    pub(crate) name: Option<Token>,
    pub(crate) options: Vec<(Expr, Option<Token>)>,
}

impl Compile for Var {
    type Output = Vec<Self>;
    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Vec<Self>> {
        let mut vars = vec![];
        let mut ident = None;
        let mut value: Option<Expr> = None;
        let mut ty = Type::None;
        let rule = token.as_rule();

        match rule {
            Rule::typed_const | Rule::cnst => ty = ty.into_const(),
            Rule::typed_static | Rule::statc | Rule::static_signal => ty = ty.into_static(),
            _ => {},
        }

        let mut tokens = token.into_inner();
        let mut name = None;
        let mut options = vec![];
        while let Some(token) = tokens.next() {
            match token.as_rule() {
                Rule::ident => {
                    if let Some(ident) = ident {
                        vars.push(Var {
                            ident: token!(ident),
                            ty: if let Type::None = ty {
                                if let Some(v) = &value {
                                    if let Some(d) = v.def(ns) {
                                        match d {
                                            Definition::GlobalVar(v) |
                                            Definition::Var(v) => v.ty.clone(),
                                            Definition::Fn(f) => f.inner().ret.clone(),
                                            Definition::Model(_) |
                                            Definition::Module(_) => Type::Path(vec![], Some(Box::new(d.clone()))),
                                            _ => Type::None,
                                        }
                                    } else {
                                        Type::None
                                    }
                                } else {
                                    Type::None
                                }
                            } else {
                                ty.clone()
                            },
                            value,
                            is_directive: false,
                            val_is_signal: false,
                            name: name.clone(),
                            options: options.clone()
                        });
                        value = None;
                        name = None;
                        options.clear();
                    }
                    ident = Some(token.clone());
                }
                Rule::typ => ty = match rule {
                    Rule::typed_const | Rule::cnst => Type::compile(token, ns)?.into_const(),
                    Rule::typed_static | Rule::statc | Rule::static_signal => Type::compile(token, ns)?.into_static(),
                    _ => Type::compile(token, ns)?,
                },
                Rule::vec | Rule::expr => value = Some(Expr::compile(token, ns)?),
                Rule::display => {
                    let mut tokens = token.into_inner();
                    let next = tokens.next().unwrap();
                    name = Some(token!(next));
                    if let Some(opts) = tokens.next() {
                        let mut tokens = opts.into_inner();
                        let mut expr = None;
                        let mut opt_name = None;
                        while let Some(next) = tokens.next() {
                            match next.as_rule() {
                                Rule::expr => {
                                    if let Some(e) = expr {
                                        options.push((Expr::compile(e, ns)?, opt_name));
                                        opt_name = None;
                                    }
                                    expr = Some(next.clone());
                                }
                                Rule::double_quoted_string => opt_name = Some(token!(next)),
                                _ => unreachable!()
                            }
                        }
                        if let Some(e) = expr {
                            options.push((Expr::compile(e, ns)?, opt_name))
                        }
                    }
                }
                _ => unreachable!("{:?}", token),
            }
        }

        if let Some(ident) = ident {
            vars.push(Var {
                ident: token!(ident),
                ty,
                value,
                is_directive: false,
                val_is_signal: false,
                name,
                options
            });
        }

        for var in &mut vars {
            if var.ty.is_none() {
                if let Some(val) = &var.value {
                    if val.is_signal(ns) {
                        var.val_is_signal = true;
                    }
                }
            }
            match rule {
                Rule::param => {
                    // // TODO: uncomment it (currently it does not work for vector default values)
                    // if var.value.is_none() {
                    //     return Err(Error::new(var.ident.clone(), "A parameter should have a default value"));
                    // }
                    var.ty = var.ty.clone().into_param()
                },
                Rule::typed_var | Rule::var => var.ty = var.ty.clone(),
                Rule::typed_static | Rule::statc => var.ty = var.ty.clone().into_static(),
                Rule::typed_const | Rule::cnst => var.ty = var.ty.clone().into_const(),
                Rule::signal => var.ty = var.ty.clone().into_signal(),
                Rule::static_signal => var.ty = var.ty.clone().into_signal().into_static(),
                Rule::pub_var => var.ty = var.ty.clone().into_pub(),
                Rule::pub_const => var.ty = var.ty.clone().into_const().into_pub(),
                Rule::pub_signal => var.ty = var.ty.clone().into_signal().into_pub(),
                Rule::mutable => var.ty = Type::Mutable,
                _ => unreachable!("{:?}", rule)
            }
            // ns.scope().borrow_mut().defs.push((var.ident.clone(), Definition::Var(var.clone())));
        }

        // Add all variables into the current scope
        Ok(vars)
    }

    fn header(&self) -> Lines {
        if self.is_directive {
            Lines::new()
        } else {
            self.def("") << ";"
        }
    }

    fn source(&self, ns: &str) -> Lines {
        if self.is_directive {
            Lines::new()
        } else {
            let lines = self.def(ns);
            if let Some(value) = &self.value {
                if self.ty.is_mutable() {
                    lines << " = " << value.source(ns) << ".mut(__current_process__);"
                } else {
                    lines << " = " << value.source(ns) << ";"
                }
            } else {
                lines << ";"
            }
        }
    }

    fn specialize(mut self, generics: &mut HashMap<Token, Type>, ns: &NameSpace) -> Self {
        self.ty = self.ty.specialize(generics, ns);
        self
    }

    fn goto_def(&self) -> Option<Token> {
        Some(self.ident.clone())
    }
}

impl Var {
    pub fn r#type(&self, ns: &str) -> Lines {
        let res = if self.ty.wrapped_in_signal() {
            self.ty.or("uint8_t")
        } else if self.val_is_signal {
            self.ty.or("auto&")
        } else {
            self.ty.or("auto")
        };
        if ns.is_empty() {
            Lines::new() << res.header() << " "
        } else {
            Lines::new() << res.drop_static().header() << " " << ns
        }
    }

    pub fn def(&self, ns: &str) -> Lines {
        if let Some(Type::Array(_, e)) = self.ty.as_array() {
            self.r#type(ns) << &self.ident << "[" << e.header() << "]"
        } else {
            self.r#type(ns) << &self.ident
        }
    }

    pub fn for_var(ident: Token) -> Self {
        Self { ident, ty: Type::None, value: None, is_directive: false, val_is_signal: false, name: None, options: vec![] }
    }
}