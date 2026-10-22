use std::collections::HashMap;
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Enum {
    ident: Token,
    options: HashMap<Token, Type>
}

impl Compile for Enum {
    type Output = Self;

    fn compile(_token: Pair<Rule>, _ns: &NameSpace) -> Result<Self::Output> {
        todo!()
    }

    fn goto_def(&self) -> Option<Token> {
        todo!()
    }
}