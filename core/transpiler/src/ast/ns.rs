
use std::{time::SystemTime, rc::Rc, cell::RefCell, collections::HashSet};

use super::{*, lines::Lines};
use crate::*;

pub type NameSpaceId = Token;

#[derive(Clone, Debug, Serialize, Deserialize, Eq)]
pub struct NameSpacePath(pub(crate) Vec<NameSpaceId>);

impl PartialEq for NameSpacePath {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Hash for NameSpacePath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl NameSpacePath {
    pub fn as_str(&self) -> String {
        self.0.iter().map(|t| t.as_str()).collect::<Vec<&str>>().join("::")
    }

    fn push(&mut self, id: NameSpaceId) {
        self.0.push(id)
    }

    fn pop(&mut self) -> Option<Token> {
        self.0.pop()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn current(&self) -> &Token {
        self.0.last().unwrap()
    }

    pub fn header_file(&self) -> String {
        format!("{}.h", self.0.iter().map(|t| t.as_str()).collect::<Vec<&str>>().join("/"))
    }
}

impl From<&str> for NameSpacePath {
    fn from(ns: &str) -> Self {
        Self(ns.split("::").map(|s| Token::from(s)).collect())
    }
}

/// A type to track the namespace
/// 
/// #Example
/// 
/// ```text
///                          | NameSpace | Path        | Declarations
/// -------------------------+-----------+-------------+------------------------
///                          | top       |             |
/// let a: int = 0;          |           | top::a      | a
/// machine m1 {             | m1        | top::m1     | a,self
///     fn f1(k: int) {      | stacked   |             | a,self,k,f1
///         if test {        | stacked   |             | a,self,k,f1
///             lat v = 10;  |           |             | a,self,k,f1,v
///         }                |           |             | a,self,k,f1
///     }                    | m1        |             | a,self
/// }                        | top       |             | a,self
/// ```
/// 
/// * stacked namespaces are temporary only for compile time
/// 
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scope {
    pub(crate) nid: NameSpacePath,
    pub(crate) defs: Vec<(Token, Definition)>,
    pub(crate) alias: HashMap<Token, Definition>,
}

impl Default for Scope {
    fn default() -> Self {
        Self { nid: NameSpacePath::from("{unnamed}"), defs: vec![], alias: HashMap::new() }
    }
}

impl Scope {
    fn new(nid: NameSpacePath) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Scope { nid, defs: Vec::new(), alias: HashMap::new() }))
    }

    pub fn compile(mut tokens: Pairs<Rule>, ns: &NameSpace) -> Result<()> {
        let mut machines = vec![];
        while let Some(token) = tokens.next() {
            match token.as_rule() {
                Rule::global_var => {
                    let mut tokens = token.into_inner();
                    let mut token = tokens.next().unwrap();
                    let mut is_directive = false;
                    if token.as_rule() == Rule::directive {
                        is_directive = true;
                        token = tokens.next().unwrap();
                    }
                    let vars = Var::compile(token, ns)?;
                    for mut var in vars {
                        var.is_directive = is_directive;
                        ns.scope().borrow_mut().defs.push((var.ident.clone(), Definition::GlobalVar(var)));
                    }
                }
                Rule::import => ns.import(
                    Import::compile(token.clone(), ns)?
                        .expect(&format!("No such file or directory '{}'", token.into_inner().next().unwrap().as_str())),
                    ns.include_path.clone())?,
                Rule::EOI => {}
                Rule::machine => machines.push(token.clone()),
                _ => {
                    let (t, def) = Definition::compile(token, ns)?;
                    ns.scope().borrow_mut().defs.push((t, def));
                }
            }
        }
        for machine in machines {
            let (t, def) = Definition::compile(machine, ns)?;
            ns.scope().borrow_mut().defs.push((t, def));
        }
        Ok(())
    }
    
    pub fn header(&self) -> Lines {
        let mut lines = Lines::new();
        for (_, def) in &self.defs {
            lines.write_lines(def.header());
        }
        lines
    }
    
    pub fn source(&self, ns: &str) -> Lines {
        let mut lines = Lines::new();
        for (_, def) in &self.defs {
            lines.write_lines(def.source(ns));
        }
        lines
    }
}

impl Scope {
    pub fn find_def(&self, id: &Token) -> Option<&Definition> {
        for (def_id, def) in self.defs.iter().rev() {
            if id == def_id {
                return Some(def);
            }
        }
        None
    }
}

struct Import;

impl Compile for Import {
    type Output = Option<(String, bool, NameSpacePath)>;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output> {
        let mut tokens = token.into_inner();
        let mut filepath = String::new();
        let mut import_all = false;
        let mut prefix = NameSpacePath(vec![]);
        while let Some(token) = tokens.next() {
            if token.as_rule() == Rule::all {
                import_all = true;
            } else {
                filepath += &format!("/{}", token.as_str());
                prefix = NameSpacePath(vec![(token!(token))]);
            }
        }
        for inc in &ns.include_path {
            let filepath = format!("{}{}.sm", inc, filepath);
            if std::path::PathBuf::from(&filepath).exists() {
                return Ok(Some((filepath, import_all, prefix)));
            }
        }
        Ok(None)
    }

    fn goto_def(&self) -> Option<Token> {
        None
    }
}

pub struct NameSpace {
    // The compiler walks the tree through `&NameSpace`, so the parts that change
    // while walking use interior mutability.
    src: RefCell<AST>,
    pub(crate) nid: RefCell<NameSpacePath>,
    stack: RefCell<Vec<Rc<RefCell<Scope>>>>,
    unnamed_scopes: RefCell<Vec<Rc<RefCell<Scope>>>>,
    include_path: Vec<String>,
    include: RefCell<Vec<String>>,  // Include statements
    compiled_files: Rc<RefCell<HashSet<String>>>,  // Include statements
}

impl NameSpace {    
    pub fn scoped<T, F: std::ops::FnOnce(Rc<RefCell<Scope>>)->T>(&self, id: Option<Token>, f: F) -> T {
        self.new_scope(id.clone());
        let res = f(self.scope());
        self.stack.borrow_mut().pop();
        if id.is_some() { self.nid.borrow_mut().pop(); } else { self.unnamed_scopes.borrow_mut().pop(); }
        res
    }

    fn new_scope(&self, id: Option<Token>) {
        if let Some(id) = &id {
            let nid = {
                let mut nid = self.nid.borrow_mut();
                nid.push(id.clone());
                nid.clone()
            };
            let scope = {
                let mut src = self.src.borrow_mut();
                src.all.insert(nid.clone(), Scope::new(nid.clone()));
                src.all.get(&nid).unwrap().clone()
            };
            self.stack.borrow_mut().push(scope);
        } else {
            let mut nid = self.nid.borrow().clone();
            nid.push(Token::from("{unnamed}"));
            let scope = Scope::new(nid);
            self.unnamed_scopes.borrow_mut().push(scope.clone());
            self.stack.borrow_mut().push(scope);
        }
    }

    pub fn scope(&self) -> Rc<RefCell<Scope>> {
        self.stack.borrow().last().unwrap().clone()
    }

    pub fn new_unnamed_scope(&self, def: &Definition) -> bool {
        if let Some(scope) = def.scope(self) {
            let scope = Rc::new(RefCell::new(scope));
            self.unnamed_scopes.borrow_mut().push(scope.clone());
            self.stack.borrow_mut().push(scope);
            return true;
        }
        false
    }

    pub fn new_named_scope(&self, path: &str) -> bool {
        let nid = NameSpacePath::from(path);
        let scope = self.src.borrow().all.get(&nid).cloned();
        if let Some(scope) = scope {
            self.unnamed_scopes.borrow_mut().push(scope.clone());
            self.stack.borrow_mut().push(scope);
            return true;
        }
        false
    }

    pub fn drop_unnamed_scope(&self) {
        self.stack.borrow_mut().pop();
        self.unnamed_scopes.borrow_mut().pop();
    }

    pub fn find_def<T: Default, F: FnOnce(&Definition)->T>(&self, id: &Token, f: F) -> T {

        if let Some(def) = self.scope().borrow().find_def(id) {
            return f(def);
        }

        let stack = self.stack.borrow().clone();
        for scope in stack.iter().rev() {
            if let Some(def) = scope.borrow().find_def(id) {
                return f(def);
            }
        }

        T::default()
    }

    fn import(&self, context: (String, bool, NameSpacePath), include_path: Vec<String>) -> Result<()> {
        let full_path = std::fs::canonicalize(&context.0).unwrap();
        let mut filename = full_path.clone();
        filename.set_extension("");
        let filename = filename.file_name().unwrap().to_str().unwrap();
        let full_path = full_path.to_str().unwrap();
        if self.compiled_files.borrow().contains(full_path) {
            return Ok(());
        }
        let mut src = Source::compile(full_path, include_path, self.compiled_files.clone(), false)?;
        let mut header_file = std::path::PathBuf::from(full_path);
        header_file.set_extension("h");
        self.include.borrow_mut().push(header_file.to_str().unwrap().to_string());
        self.compiled_files.borrow_mut().insert(full_path.to_owned());
        for (nid, scope) in &mut src.all {
            if context.1 {
                if nid.len() == 1 && nid.0[0].as_str() == filename {
                    let s = self.scope();
                    let mut curr = s.borrow_mut();
                    for def in &scope.defs {
                        curr.defs.push(def.clone())
                    }
                }
            } else {
                if self.src.borrow_mut().all.insert(scope.nid.clone(), Rc::new(RefCell::new(scope.clone()))).is_some() {
                    panic!("Duplicate namespaces {}", nid.as_str());
                }
            }
        }
        Ok(())
    }
}

/// A source file
#[derive(Clone, Serialize, Deserialize)]
pub struct Source {
    pub(crate) all: HashMap<NameSpacePath, Scope>,
    pub(crate) created: SystemTime,  // Time that the source file is compiled
    pub(crate) libs: Vec<NameSpacePath>,  // Imported Libraries
    pub(crate) include: Vec<String>,  // Imported Libraries
}

pub struct AST {
    pub(crate) all: HashMap<NameSpacePath, Rc<RefCell<Scope>>>,
}

impl Source {
    pub fn save(&self, path: &str) -> std::io::Result<()> {
        std::fs::write(path, bincode::serialize(self).unwrap())
    }

    pub fn load(path: &str) -> Option<Self> {
        let encoded = std::fs::read(path).expect("unable to read file");
        bincode::deserialize(&encoded[..]).ok()
    }

    pub fn header(&self, top: &NameSpacePath, lib_interface: bool) -> String {
        let mut res = "#pragma once\n".to_owned();
        res += "#include <eztrading.h>\n";
        for inc in &self.include {
            res += &format!("#include <{}>\n", inc);
        }
        res += "\n";
        let scope = self.all.get(top).unwrap();
        res += &format!("namespace {} {{", top.current().as_str());
        res += &scope.header().print("");
        res += "\n}";
        if lib_interface {
            res += "\nextern \"C\" sml::machine *launch(uint32_t run_id, Scheduler *scheduler, const char *model_name, char *params_json);";
            res += "\nextern \"C\" void contents(__contents::list *list);";
            res += "\nextern \"C\" void contents_json(char **json, size_t *len);\n";
        }
        res
    }
    
    fn generate_setup(&self) -> String {
        let mut res = String::new();
        res += "\nextern \"C\" sml::machine *launch(uint32_t run_id, Scheduler *scheduler, const char *model_name, char *params_json) {\n";
        for (n, scope) in &self.all {
            for (t, def) in &scope.defs {
                if let Definition::Model(_) = def {
                    res += &format!("    if(!strcmp(model_name, \"{}::{}\")) {{\n", n.as_str(), t.as_str());
                    res += &format!("        return new {}::{}(run_id, scheduler, params_json);\n", n.as_str(), t.as_str());
                    res += "    }\n";
                }
            }
        }
        res += "    return nullptr;\n";
        res += "}\n";
        res += "\nextern \"C\" void contents(__contents::list *list) {\n";
        for (n, scope) in &self.all {
            for (t, def) in &scope.defs {
                if let Definition::Model(_) = def {
                    res += &format!("    list->add({}::{}::__get_contents());\n", n.as_str(), t.as_str());
                }
            }
        }
        res += "}\n";
        res += "\nextern \"C\" void contents_json(char **json, size_t *size) {\n";
        res += "    __contents::list lst;\n";
        res += "    contents(&lst);\n";
        res += "    std::string json_str = lst.to_json();\n";
        res += "    *size = json_str.size();\n";
        res += "    *json = new char[*size];\n";
        res += "    strcpy(*json, json_str.data());\n";
        res += "}\n";
        res
    }

    pub fn source(&self, top: &NameSpacePath, lib_interface: bool) -> String {
        let mut res = String::new();
        let (ns, scope) = self.all.get_key_value(top).unwrap();
        let mut st = ns.current().as_str().to_owned();
        if !st.is_empty() {
            st = format!("{}::", st);
        }
        res += &format!("#include \"{}\"\n", ns.header_file());
        res += "#include <eztrading.h>\n\n";
        res += &format!("using namespace {};\n", ns.current().as_str());
        res += &scope.source(&st).print("");
        if lib_interface {
            res += &self.generate_setup();
        }
        res
    }

    fn from_ast(ast: AST) -> Self {
        let mut all: HashMap<NameSpacePath, Scope> = HashMap::new();
        for (k, scope) in ast.all {
            all.insert(k, scope.borrow().clone());
        }
        Self {
            all,
            created: SystemTime::now(),
            libs: vec![],
            include: vec![]
        }
    }

    // fn to_ast(self) -> AST {
    //     let mut all: HashMap<NameSpacePath, Rc<RefCell<Scope>>> = HashMap::new();
    //     for (k, scope) in self.all {
    //         all.insert(k, Rc::new(RefCell::new(scope.clone())));
    //     }
    //     AST {
    //         all
    //     }
    // }

    pub fn compile(path: &str, mut include_path: Vec<String>, compiled_files: Rc<RefCell<HashSet<String>>>, lib_interface: bool) -> Result<Self> {
        let mut obj_file = std::path::PathBuf::from(path);
        let mut hdr_file = obj_file.clone();
        let mut src_file = obj_file.clone();
        let mut aux_file = obj_file.clone();
        aux_file.set_extension("");
        obj_file.set_extension("smo");
        hdr_file.set_extension("h");
        src_file.set_extension("cpp");
        let module = aux_file.file_name().unwrap().to_str().unwrap();
        let obj_path = obj_file.clone().into_os_string().into_string().unwrap();
        if obj_file.exists() {
            if let Some(res) = Self::load(&obj_path) {
                let md = std::fs::metadata(path).expect("unable to open file");
                if let Ok(mtime) = md.modified() {
                    if mtime <= res.created {
                        // The source file has not been modified -> load
                        // TODO: check included files, too.
                        // return res;
                    }
                }
            }
        }
        let file = std::fs::read_to_string(path).map_err(|e| Error {
            token: Token::default(),
            what: format!("cannot read file {}: {}", path, e),
        })?;
        let tokens = EZTParser::parse(Rule::code, &file).map_err(|e| {
            let (row, col) = match e.line_col {
                pest::error::LineColLocation::Pos(p) | pest::error::LineColLocation::Span(p, _) => p,
            };
            let mut token = Token::default();
            token.src.row = row;
            token.src.col = col;
            Error {
                token,
                what: format!("syntax error: {}", e.variant.message()),
            }
        })?;
        let mut ast = AST {
            all: HashMap::new(),
            // current: NameSpacePath::new(&["top"]),
            // created: SystemTime::now(),
        };
        let top_ns = NameSpacePath::from(module);
        if let Some(dir) = obj_file.parent() {
            let mut dir = dir.to_str().unwrap().to_owned();
            if let Ok(curr) = std::fs::canonicalize(&format!("./{}", dir)) {
                dir = curr.to_str().unwrap().to_owned();
            }
            include_path.insert(0, dir);
        }

        ast.all.insert(top_ns.clone(), Scope::new(top_ns.clone()));
        let src = ast;
        let ns = NameSpace {
            src: RefCell::new(src),
            nid: RefCell::new(top_ns.clone()),
            unnamed_scopes: RefCell::new(vec![]),
            stack: RefCell::new(vec![]),
            include_path,
            include: RefCell::new(vec![]),
            compiled_files: compiled_files.clone()
        };
        let top = ns.src.borrow().all.get(&top_ns).unwrap().clone();
        ns.stack.borrow_mut().push(top);
        Scope::compile(tokens, &ns)?;
        let mut source = Self::from_ast(ns.src.into_inner());
        if source.save(&obj_path).is_ok() {
            println!("Object file is written to {} successfully!", obj_path)
        } else {
            println!("Failed to create an object file at {}.", obj_path)
        }
        source.include = ns.include.borrow().clone();
        std::fs::write(hdr_file, source.header(&top_ns, lib_interface)).expect("unable to write .h");
        std::fs::write(src_file, source.source(&top_ns, lib_interface)).expect("unable to write .cpp");
        Ok(source)
    }
}