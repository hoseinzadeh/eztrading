use std::cell::RefCell;
use colored::Colorize;
use com::{
    failed,
    net::{ConnectionError, ConnectionInternal, Packet},
    status::{ErrorCode, Status},
};
use crossbeam_channel::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fmt::{Debug, Display},
    ops::Index,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    thread,
    thread::JoinHandle,
    time::Duration,
    vec, net::SocketAddr,
};

use crate::{
    ci::{CommandInterface, DataSrvCI},
    errprintln,
};

#[derive(Clone)]
pub struct Prompt(pub Arc<RwLock<String>>);

impl ToString for Prompt {
    fn to_string(&self) -> String {
        self.0.read().unwrap().clone()
    }
}


#[derive(Clone)]
pub struct Terminal {
    // Commands are replaced while one of them runs (e.g. `disconnect`), so the
    // registry uses interior mutability and `execute` runs a clone.
    all: RefCell<Vec<Executable>>,
    map: RefCell<HashMap<String, usize>>,
    shorts: RefCell<HashMap<String, usize>>,
    pub prompt: Prompt,
    server: Arc<Mutex<Option<ConnectionInternal>>>,
    stdout: Sender<ConnectionError>,
    threads: Arc<Mutex<Vec<JoinHandle<Status>>>>,
    working: Arc<AtomicBool>,
}

impl Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.all
            .borrow()
            .iter()
            .map(|v| &v.cmd)
            .collect::<Vec<&Command>>()
            .fmt(f)
    }
}

impl Terminal {
    pub fn new() -> Self {
        let prompt = Arc::new(RwLock::new("> ".to_owned()));
        let pmt = prompt.clone();
        let (stdout, stdout_rx) = unbounded();
        let server = Arc::new(Mutex::new(None));
        let working = Arc::new(AtomicBool::new(true));
        let server_clone = server.clone();
        let working_clone = working.clone();
        let builder = thread::Builder::new().name("terminal".into());
        builder.spawn(move || {
            while working_clone.load(Ordering::Relaxed) {
                match stdout_rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(e) => match e {
                        ConnectionError::Disconnected(tcp) => {
                            let mut pmt = pmt.write().unwrap();
                            if *pmt != "> " {
                                *pmt = "> ".to_owned();
                                eprint!(
                                    "\nDisconnected from {}\n> ",
                                    tcp.map(|tcp| tcp.peer_addr().unwrap().to_string())
                                        .unwrap_or("the server".into())
                                )
                            }
                            *server_clone.lock().unwrap() = None;
                        }
                        ConnectionError::Other(_, e) => errprintln!("Error: {e}"),
                    },
                    Err(RecvTimeoutError::Timeout) => (),
                    Err(_) => break,
                }
            }
        }).unwrap();

        Self {
            all: RefCell::new(vec![]),
            map: RefCell::new(HashMap::new()),
            shorts: RefCell::new(HashMap::new()),
            prompt: Prompt(prompt),
            server,
            stdout,
            threads: Arc::new(Mutex::new(vec![])),
            working,
        }
    }

    pub fn clear_commands(&self) {
        self.all.borrow_mut().clear();
        self.map.borrow_mut().clear();
        self.shorts.borrow_mut().clear();
    }

    pub fn intake_commands(&self, commands: Vec<Executable>) -> Result<(), Status> {
        let mut commands = VecDeque::<Executable>::from(commands);
        while let Some(cmd) = commands.pop_front() {
            if let Err(s) = self.add_command(cmd) {
                return Err(s);
            }
        }
        Ok(())
    }

    pub fn add_command(&self, exe: Executable) -> Result<(), Status> {
        let len = self.all.borrow().len();
        if let Some(sh) = &exe.cmd.short {
            if self.shorts.borrow_mut().insert(sh.clone(), len).is_some() {
                return Err(failed!(format!("Duplicate short command '{}'", sh)));
            }
        }
        let cmd = exe.cmd.ident.clone();
        if self.map.borrow_mut().insert(cmd.clone(), len).is_some() {
            return Err(failed!(format!("Duplicate command '{}'", cmd)));
        }
        self.all.borrow_mut().push(exe);
        Ok(())
    }

    pub fn print_commands(&self) {
        println!("AVAILABLE COMMANDS:");
        for exe in self.all.borrow().iter() {
            exe.cmd.help();
        }
    }

    pub fn start_async_recv<F: Fn(Packet, SocketAddr) + Send + Sync + 'static>(&self, f: F) {
        let mut threads = self.threads.lock().unwrap();
        if let Some(conn) = &*self.server.lock().unwrap() {
            threads.push(conn.start_async_recv(f));
        }
    }

    pub fn connect(&self, server: &str) -> Result<(), Status> {
        let mut srv = self.server.lock().unwrap();
        if srv.is_none() {
            let (mut threads, conn) = ConnectionInternal::connect(
                server,
                Some(self.stdout.clone()),
                self.working.clone(),
                None
            )?;
            *srv = Some(conn);
            self.threads.lock().unwrap().append(&mut threads);
            *self.prompt.0.write().unwrap() = server.to_owned();
            Ok(())
        } else {
            Err(failed!("Already connected", ErrorCode::Network))
        }
    }

    pub fn connected(&self) -> bool {
        let srv = self.server.lock().unwrap();
        if let Some(server) = &*srv {
            server.connected()
        } else {
            false
        }
    }

    pub fn disconnect(&self) -> Result<(), Status> {
        let ret = {
            let mut srv = self.server.lock().unwrap();
            if let Some(server) = &mut *srv {
                self.prompt.0.write().unwrap().clear();
                server.disconnect()?;
                self.clear_commands();
                self.intake_commands(DataSrvCI::default_commands())?;
                *srv = None;
                Ok(())
            } else {
                Err(failed!("Not connected", ErrorCode::Network))
            }
        };

        while let Some(t) = self.threads.lock().unwrap().pop() {
            let _ = t.join();
        }
        ret
    }

    pub fn request<T, R>(&self, data: T) -> Result<R, Status>
    where
        T: From<Packet> + Serialize,
        R: From<Packet> + Serialize,
    {
        let srv = self.server.lock().unwrap();
        if let Some(server) = &*srv {
            server.request(data)
        } else {
            Err(failed!("No connection", ErrorCode::Network))
        }
    }

    pub fn receive(&self) -> Result<Packet, Status> {
        let srv = self.server.lock().unwrap();
        if let Some(server) = &*srv {
            server.recv()
        } else {
            Err(failed!("No connection", ErrorCode::Network))
        }
    }

    pub fn send<T: Serialize>(&self, data: T) -> Result<(), Status> {
        let srv = self.server.lock().unwrap();
        if let Some(server) = &*srv {
            server.send(data)
        } else {
            Err(failed!("No connection", ErrorCode::Network))
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Command {
    ident: String,
    short: Option<String>,
    help: String,
    args: Args,
}

impl Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{} {}",
            self.ident.bold().red(),
            if let Some(s) = &self.short {
                format!("|{}", s.bold().red())
            } else {
                String::new()
            },
            self.args
                .list
                .iter()
                .map(|op| format!("[{:?}]", op))
                .collect::<Vec<String>>()
                .join(" ")
        )
    }
}

impl Command {
    pub fn help(&self) {
        println!("  {}", self.help.italic().truecolor(128, 128, 128));
        println!("  {:?}\n", self);
    }

    fn parse_args(&self, tokens: &mut VecDeque<String>) -> Result<Args, Status> {
        Self::parse_args_internal(&self.args, tokens, true)
    }

    fn parse_args_internal(
        self_args: &Args,
        tokens: &mut VecDeque<String>,
        error_unkown_arg: bool,
    ) -> Result<Args, Status> {
        let mut args = Args::new();
        while let Some(token) = tokens.pop_front() {
            let mut found = false;
            for arg in &self_args.list {
                if arg.name == token {
                    found = true;
                    match arg.clone().set(tokens) {
                        Ok(new) => args.list.push(new),
                        Err(e) => return Err(e),
                    }
                    break;
                }
            }
            if !found {
                if error_unkown_arg {
                    found = false;
                    tokens.push_front(token.clone());
                    for arg in &self_args.list {
                        if arg.name.is_empty() {
                            found = true;
                            match arg.clone().set(tokens) {
                                Ok(new) => args.list.push(new),
                                Err(e) => return Err(e),
                            }
                        }
                    }

                    if !found {
                        return Err(failed!(format!("Unknown arg '{}'", token)));
                    }
                } else {
                    tokens.push_front(token);
                    break;
                }
            }
        }
        Ok(args)
    }
}

/// Executable command
#[derive(Clone)]
pub struct Executable {
    cmd: Command,
    exec: Option<fn(&Args, &Terminal, &Command) -> Result<i32, Status>>,
}

impl Executable {
    pub fn new(
        ident: &str,
        short: Option<&str>,
        help: &str,
        args: Args,
        exec: fn(&Args, &Terminal, &Command) -> Result<i32, Status>,
    ) -> Self {
        Self {
            cmd: Command {
                ident: ident.to_owned(),
                short: short.map(|s| s.to_owned()),
                help: help.to_owned(),
                args,
            },
            exec: Some(exec),
        }
    }

    pub fn execute(&self, args: &Args, cmd_list: &Terminal) -> Result<i32, Status> {
        if let Some(local_exec) = self.exec {
            let args = args.clone().add_defaults(&self.cmd.args);
            (local_exec)(&args, cmd_list, &self.cmd)
        } else {
            todo!()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Args {
    list: Vec<Arg>,
}

impl Args {
    pub fn new() -> Self {
        Self { list: vec![] }
    }

    pub fn add(mut self, arg: Arg) -> Self {
        self.list.push(arg);
        self
    }

    fn has_sub_arg(&self, name: &str) -> bool {
        for arg in &self.list {
            if arg.name == name {
                return true;
            }
        }
        false
    }

    fn find_subs(&self, name: &str) -> Option<&Args> {
        for arg in &self.list {
            if arg.name == name {
                return Some(&arg.args);
            }
        }
        None
    }

    pub fn add_defaults(mut self, defs: &Args) -> Self {
        for d in &defs.list {
            if !self.has_sub_arg(&d.name) {
                if d.value.has_default() {
                    self.list.push(d.clone());
                }
            }
        }
        for arg in &mut self.list {
            arg.args = Box::new(arg.clone().args.add_defaults(defs.find_subs(&arg.name).unwrap()));
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<(&Value, &Args)> {
        for arg in &self.list {
            if arg.name == name {
                return Some((&arg.value, arg.args.as_ref()));
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

impl IntoIterator for Args {
    type Item = Arg;

    type IntoIter = std::vec::IntoIter<Arg>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.into_iter()
    }
}

impl<'a> IntoIterator for &'a Args {
    type Item = &'a Arg;

    type IntoIter = std::slice::Iter<'a, Arg>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.iter()
    }
}

impl Index<usize> for Args {
    type Output = Arg;

    fn index(&self, index: usize) -> &Self::Output {
        self.list.index(index)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Arg {
    pub name: String,
    pub value: Value,
    pub args: Box<Args>,
}

impl Debug for Arg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}{}",
            self.name.cyan(),
            self.value,
            if !self.args.is_empty() {
                let mut ret = " [".to_owned();
                ret += self
                    .args
                    .list
                    .iter()
                    .map(|a| format!("{:?}", a))
                    .collect::<Vec<String>>()
                    .join(" ")
                    .as_str();
                ret += "]";
                ret
            } else {
                String::new()
            }
        )
    }
}

impl Arg {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: Value::Flag,
            args: Box::new(Args::new()),
        }
    }

    pub fn new_str<S: ToString>(name: S, default: Option<S>) -> Self {
        Self {
            name: name.to_string(),
            value: Value::Str(default.map(|s| s.to_string())),
            args: Box::new(Args::new()),
        }
    }

    pub fn new_int(name: &str, default: Option<i64>) -> Self {
        Self {
            name: name.to_owned(),
            value: Value::Int(default),
            args: Box::new(Args::new()),
        }
    }

    pub fn new_float(name: &str, default: Option<f32>) -> Self {
        Self {
            name: name.to_owned(),
            value: Value::Float(default),
            args: Box::new(Args::new()),
        }
    }

    pub fn add(mut self, arg: Arg) -> Self {
        if !self.args.is_empty() {
            self.args.list.push(arg);
        } else {
            self.args = Box::new(Args { list: vec![arg] });
        }
        self
    }

    pub fn set(mut self, tokens: &mut VecDeque<String>) -> Result<Self, Status> {
        if let Some(val) = tokens.pop_front() {
            match &mut self.value {
                Value::Str(t) => *t = Some(val),
                Value::Int(t) => {
                    if let Ok(num) = val.parse::<i64>() {
                        *t = Some(num)
                    } else {
                        return Err(failed!(format!("Expected a number, found '{}'", val)));
                    }
                }
                Value::Float(t) => {
                    if let Ok(num) = val.parse::<f32>() {
                        *t = Some(num)
                    } else {
                        return Err(failed!(format!("Expected a number, found '{}'", val)));
                    }
                }
                Value::Flag => tokens.push_front(val),
            }
        } else if self.value != Value::Flag {
            return Err(failed!("Expected a value"));
        }
        if !self.args.is_empty() {
            if let Ok(subs) = Command::parse_args_internal(self.args.as_ref(), tokens, false) {
                self.args = Box::new(subs);
            }
        }
        Ok(self)
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Value {
    Str(Option<String>),
    Int(Option<i64>),
    Float(Option<f32>),
    Flag,
}

impl Value {
    pub fn has_default(&self) -> bool {
        match self {
            Value::Str(Some(_)) => true,
            Value::Int(Some(_)) => true,
            Value::Float(Some(_)) => true,
            _ => false,
        }
    }
}

impl Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Str(_) => f.write_str("Str"),
            Self::Int(_) => f.write_str("Int"),
            Self::Float(_) => f.write_str("Float"),
            Self::Flag => Ok(()),
        }
    }
}

impl TryInto<i64> for Value {
    type Error = Status;

    fn try_into(self) -> Result<i64, Self::Error> {
        match self {
            Self::Int(Some(val)) => Ok(val),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<u64> for Value {
    type Error = Status;

    fn try_into(self) -> Result<u64, Self::Error> {
        match self {
            Self::Int(Some(val)) => Ok(val as u64),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<i64> for &Value {
    type Error = Status;

    fn try_into(self) -> Result<i64, Self::Error> {
        match self {
            Value::Int(Some(val)) => Ok(*val),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<u64> for &Value {
    type Error = Status;

    fn try_into(self) -> Result<u64, Self::Error> {
        match self {
            Value::Int(Some(val)) => Ok(*val as u64),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<i32> for Value {
    type Error = Status;

    fn try_into(self) -> Result<i32, Self::Error> {
        match self {
            Self::Int(Some(val)) => Ok(val as i32),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<i32> for &Value {
    type Error = Status;

    fn try_into(self) -> Result<i32, Self::Error> {
        match *self {
            Value::Int(Some(val)) => Ok(val as i32),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<u32> for Value {
    type Error = Status;

    fn try_into(self) -> Result<u32, Self::Error> {
        match self {
            Self::Int(Some(val)) => Ok(val as u32),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<u32> for &Value {
    type Error = Status;

    fn try_into(self) -> Result<u32, Self::Error> {
        match *self {
            Value::Int(Some(val)) => Ok(val as u32),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<String> for &Value {
    type Error = Status;

    fn try_into(self) -> Result<String, Self::Error> {
        match self {
            Value::Str(Some(val)) => Ok(val.clone()),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl<'a> TryInto<&'a str> for &'a Value {
    type Error = Status;

    fn try_into(self) -> Result<&'a str, Self::Error> {
        match self {
            Value::Str(Some(val)) => Ok(val),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<f32> for Value {
    type Error = Status;

    fn try_into(self) -> Result<f32, Self::Error> {
        match self {
            Self::Float(Some(val)) => Ok(val),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl TryInto<f32> for &Value {
    type Error = Status;

    fn try_into(self) -> Result<f32, Self::Error> {
        match *self {
            Value::Float(Some(val)) => Ok(val),
            _ => Err(failed!("Unable to convert"))
        }
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Str(arg0) => write!(
                f,
                "=<string>{}",
                arg0.as_ref()
                    .map_or(String::new(), |f| format!("(def: {})", f))
            ),
            Self::Int(arg0) => write!(
                f,
                "=<integer>{}",
                arg0.as_ref()
                    .map_or(String::new(), |f| format!("(def: {})", f))
            ),
            Self::Float(arg0) => write!(
                f,
                "=<number>{}",
                arg0.as_ref()
                    .map_or(String::new(), |f| format!("(def: {})", f))
            ),
            Self::Flag => Ok(()),
        }
    }
}

pub fn execute(cmd: &str, cmd_list: &Terminal) -> Result<i32, Status> {
    fn tokenize(str: &str) -> VecDeque<String> {
        let mut res = vec![];
        let mut cur = String::new();
        let mut quote = false;
        let mut esc = false;
        for c in str.chars() {
            if !esc && c == '\\' {
                esc = true;
                continue;
            } else if esc {
                if c == 'n' {
                    cur.push('\n')
                } else {
                    cur.push(c)
                }
                esc = false;
            } else if quote && c == '"' {
                quote = false
            } else if !quote && (c == '=' || c == ' ' || c == '\t' || c == '\n') {
                if !cur.is_empty() {
                    res.push(cur.clone());
                    cur.clear();
                }
            } else if c == '"' {
                quote = true;
            } else {
                cur.push(c)
            }
        }
        if !cur.is_empty() {
            res.push(cur.clone());
        }
        res.into()
    }

    let mut tokens = tokenize(cmd);
    if !tokens.is_empty() {
        let cmd_name = tokens.pop_front().unwrap();
        let mut cmd_idx = usize::MAX;
        if let Some(idx) = cmd_list.map.borrow().get(&cmd_name) {
            cmd_idx = *idx;
        } else {
            if let Some(long) = cmd_list.shorts.borrow().get(&cmd_name) {
                cmd_idx = *long;
            }
        }

        if cmd_idx == usize::MAX {
            Err(failed!(format!("Unknown command '{}'", cmd_name)))
        } else {
            let exe = cmd_list.all.borrow()[cmd_idx].clone();
            let args = exe.cmd.parse_args(&mut tokens)?;
            exe.execute(&args, cmd_list)
        }
    } else {
        Ok(0)
    }
}
