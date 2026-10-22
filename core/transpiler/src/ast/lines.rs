use std::ops::{AddAssign, Shl};
use super::{token::Token, Compile};

pub const TAB_SIZE: usize = 3;

#[derive(Default, Clone)]
pub struct Line {
    str: String,
    tokens: Vec<Token>,
    indent: u32
}

// impl Line {
//     fn new(str: String) -> Self {
//         Line {
//             str,
//             tokens: vec![],
//             indent: 0
//         }
//     }
// }

pub struct NewLine;

#[derive(Clone)]
pub struct Lines(Vec<Line>);

impl Lines {
    pub fn new() -> Self {
        Self(vec!(Line::default()))
    }

    pub fn print(&mut self, indent: &str) -> String {
        let mut res = String::new();
        let mut indent = indent.to_owned();
        let ind = indent.len();
        // let mut lnum = 1;
        for line in &mut self.0 {
            line.indent = ind as u32;
            let out = &line.str;
            if !out.is_empty() && out != "{" {
                res += "\n";
                // lnum += 1;
            }
            let mut dec = 0;
            if out.contains("{") || out.contains("}") {
                dec = Self::should_dec(&out);
            }
            let write_after = out.starts_with("}");
            if !write_after {
                if out == "public:" || out == "protected:" || out == "private:" {
                    res += &" ".repeat(indent.len() - TAB_SIZE);
                } else if out != "{" {
                    res += &indent;
                } else {
                    res += " ";
                }
                res += out;
            }
            if dec != 0 {
                indent = " ".repeat((indent.len() as i32 + dec * TAB_SIZE as i32) as usize);
            }
            if write_after {
                res += &indent;
                res += out;
            }
        }
        res
    }

    fn should_dec(line: &str) -> i32 {
        let mut dec = 0;
        let mut p = '\0';
        let mut st = false;
        let mut ch = false;
        let mut cmt = false;
        for c in line.chars() {
            if !cmt && !ch && c == '"' && p != '\\' { st = !st; }
            if !cmt && !st && c == '\'' && p != '\\' { ch = !ch; }
            else if !st && !ch && c == '*' && p == '/' { cmt = true; }
            else if !st && !ch && c == '/' && p == '*' { cmt = false; }
            else if !st && !ch && !cmt && c == '/' && p == '/' { break; }
            else if !st && !ch && !cmt && c == '{' { dec += 1 }
            else if !st && !ch && !cmt && c == '}' { dec -= 1 }
            p = c;
        }
        dec
    }

    pub fn join(&self, sep: &str) -> Lines {
        let mut line = Line::default();
        for i in 0..self.0.len() {
            let mut add = self.0[i].clone();
            let len = line.str.len();
            for t in &mut add.tokens {
                t.dst.col += len;
            }
            line.str += &add.str;
            if i < self.0.len()-1 {
                line.str += sep;
            }
            line.tokens.append(&mut add.tokens.clone());
        }
        Lines(vec![line])
    }

    pub fn write(&mut self, rhs: &Token) -> &mut Self {
        let mut rhs = rhs.clone();
        let last = self.0.last_mut().unwrap();
        rhs.dst.col = last.str.len();
        last.str += &rhs.ident;
        last.tokens.push(rhs);
        self
    }

    pub fn write_str(&mut self, rhs: &str) -> &mut Self {
        let to_add: Vec<Line> = rhs.split_terminator('\n').map(|l| Line::from(l)).collect();
        self.write_lines(Lines(to_add))
    }

    pub fn write_lines(&mut self, mut rhs: Lines) -> &mut Self {
        if rhs.0.is_empty() {
            return self;
        }
        let last = self.0.last_mut().unwrap();
        let sh = last.str.len();
        let other = rhs.0.first_mut().unwrap();
        for t in &mut other.tokens {
            t.dst.col += sh;
        }
        last.str += &other.str;
        if !other.tokens.is_empty() {
            last.tokens.append(&mut other.tokens);
        }
        if rhs.0.len() > 1 {
            self.0.append(&mut rhs.0[1..].to_vec());
        }
        self
    }
}

impl AddAssign for Lines {
    fn add_assign(&mut self, mut rhs: Self) {
        self.0.append(&mut rhs.0)
    }
}

// impl AddAssign<&str> for Lines {
//     fn add_assign(&mut self, rhs: &str) {
//         self.0.push(rhs.trim().to_owned())
//     }
// }

// impl AddAssign<String> for Lines {
//     fn add_assign(&mut self, rhs: String) {
//         self.0.push(rhs.trim().to_owned())
//     }
// }

impl From<String> for Lines {
    fn from(input: String) -> Self {
        Lines::new() << input
    }
}

impl FromIterator<Line> for Lines {
    fn from_iter<T: IntoIterator<Item = Line>>(iter: T) -> Self {
        Self(Vec::<Line>::from_iter(iter))
    }
}

impl FromIterator<Lines> for Lines {
    fn from_iter<T: IntoIterator<Item = Lines>>(iter: T) -> Self {
        let mut res = Self::new();
        res.0.clear();
        for lines in iter {
            res += lines;
        }
        res
    }
}

impl IntoIterator for Lines {
    type Item = Line;
    type IntoIter = std::vec::IntoIter<Line>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

// impl Display for Lines {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.print(""))
//     }
// }


impl From<String> for Line {
    fn from(input: String) -> Self {
        Line { str: input, tokens: vec![], indent: 0 }
    }
}

impl From<&str> for Line {
    fn from(input: &str) -> Self {
        Line { str: input.to_owned(), tokens: vec![], indent: 0 }
    }
}

// impl Shl<&str> for &mut Line {
//     type Output = Self;

//     fn shl(self, rhs: &str) -> Self {
//         self.str += rhs;
//         self
//     }
// }

// impl Shl<Token> for &mut Line {
//     type Output = Self;

//     fn shl(self, mut rhs: Token) -> Self {
//         rhs.dst.col = self.str.len();
//         self.str += &rhs.ident;
//         self.tokens.push(rhs);
//         self
//     }
// }

impl Shl<&Token> for Lines {
    type Output = Self;

    fn shl(mut self, rhs: &Token) -> Self {
        self.write(rhs);
        self
    }
}

impl<T: ToString> Shl<T> for Lines {
    type Output = Self;

    fn shl(mut self, rhs: T) -> Self {
        self.write_str(&rhs.to_string());
        self
    }
}

impl Shl<NewLine> for Lines {
    type Output = Self;

    fn shl(mut self, _rhs: NewLine) -> Self {
        self.0.push(Line::default());
        self
    }
}

impl Shl<Lines> for Lines {
    type Output = Self;

    fn shl(mut self, rhs: Lines) -> Self {
        self.write_lines(rhs);
        self
    }
}

pub trait JoinLines {
    fn join(&self, sep: &str) -> Lines;
}

impl<F: Compile> JoinLines for Vec<F> {
    fn join(&self, sep: &str) -> Lines {
        let mut lines = Lines::new();
        for i in 0 .. self.len() {
            lines.write_lines(self[i].header());
            if i < self.len()-1 {
                lines.write_str(sep);
            }
        }
        lines
    }
}

impl JoinLines for Vec<Token> {
    fn join(&self, sep: &str) -> Lines {
        let mut lines = Lines::new();
        for i in 0..self.len() {
            lines.write(&self[i]);
            if i < self.len()-1 {
                lines.write_str(sep);
            }
        }
        lines
    }
}