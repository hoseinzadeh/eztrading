use std::fmt::Debug;

use super::*;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Span {
    pub(crate) row: usize,
    pub(crate) col: usize,
}

impl Span {
    pub fn new(span: pest::Span) -> Self {
        let pos = span.start_pos().line_col();
        Span {
            row: pos.0,
            col: pos.1,
        }
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Token {
    pub(crate) ident: String,
    pub(crate) src: Span,
    pub(crate) dst: Span,
}

impl Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.ident)
    }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        self.ident == other.ident
    }
}

impl Eq for Token {}

// impl Display for Token {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.ident)
//     }
// }

impl Hash for Token {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.ident.hash(state);
    }
}

impl Token {
    pub fn new(token: Pair<Rule>) -> Self {
        Token {
            ident: token.as_str().to_owned(),
            src: Span::new(token.as_span()),
            dst: Default::default(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.ident
    }

    // pub fn any(&self, vec: &[&str]) -> bool {
    //     for v in vec {
    //         if self.ident == *v {
    //             return true;
    //         }
    //     }
    //     false
    // }
}

impl From<&str> for Token {
    fn from(s: &str) -> Self {
        let mut t = Self::default();
        t.ident = s.to_owned();
        t
    }
}

#[macro_export]
macro_rules! token {
    ($arg:expr) => {
        Token::new($arg)
    };
}
