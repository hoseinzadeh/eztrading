use std::{hash::Hash, fmt::Display};
use serde::{Serialize, Deserialize};
use pest::iterators::{Pair, Pairs};
use std::collections::HashMap;
use lazy_static::lazy_static;
use pest::{Parser, pratt_parser::{Assoc, Op, PrattParser}};
use pest_derive::Parser;

pub mod expr;
pub mod func;
pub mod r#type;
pub mod r#enum;
pub mod r#struct;
pub mod var;
pub mod ns;
pub mod block;
pub mod token;
pub mod result;
pub mod lines;
pub mod def;

use r#struct::*;
use r#type::*;
use r#enum::*;
use expr::*;
use var::*;
use ns::*;
use func::*;
use block::*;
use token::*;
use lines::*;
use def::*;

#[derive(Debug)]
pub struct Error {
    pub token: Token,
    pub what: String
}

impl Error {
    pub fn new(token: Token, what: &str) -> Self {
        Self { token, what: what.to_owned() }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("({},{}): {}", self.token.src.row, self.token.src.col, self.what))
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Compile {
    type Output;

    fn compile(token: Pair<Rule>, ns: &NameSpace) -> Result<Self::Output>;
    fn source(&self, _ns: &str) -> Lines { unimplemented!() }
    fn header(&self) -> Lines { unimplemented!() }
    fn specialize(self, _generics: &mut HashMap<Token, Type>, _ns: &NameSpace) -> Self
    where Self: Sized { unimplemented!() }
    fn goto_def(&self) -> Option<Token>;
}


#[derive(Parser)]
#[grammar = "eztrading.pest"] // relative to src
pub struct EZTParser;

lazy_static! {
    static ref PRATT_PARSER: PrattParser<Rule> = {
        use Assoc::*;
        use Rule::*;

        PrattParser::new()
            .op(Op::infix(assign, Right) | Op::infix(eq, Right) |
                Op::infix(inline_if, Right))
            .op(Op::postfix(milli) | Op::postfix(second) | Op::postfix(minute) |
                Op::postfix(hour) | Op::postfix(day) | Op::postfix(week) |
                Op::postfix(month) | Op::postfix(year))
            .op(Op::infix(range, Left))
            .op(Op::infix(math9, Left))
            .op(Op::infix(math8, Left))
            .op(Op::infix(math7, Left))
            .op(Op::infix(math6, Left))
            .op(Op::infix(math5, Left))
            .op(Op::infix(math4, Left))
            .op(Op::infix(math3, Left))
            .op(Op::infix(math2, Left))
            .op(Op::infix(math1, Left))
            .op(Op::prefix(prefix))
            .op(Op::infix(member, Left) | Op::postfix(indx) |
                Op::postfix(pack) | Op::postfix(temp))
            .op(Op::infix(scope, Left))
    };
}