use mysql::{params, prelude::*, PooledConn, TxOpts};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{
    convert,
    status::{ErrorCode, Status},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct User {
    id: u32,
    name: String,
    lastname: Option<String>,
    email: String,
    phonenumber: String,
    joined: String,
    plan: Plan,
}

impl User {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn new(
        name: String,
        lastname: Option<String>,
        email: String,
        phonenumber: String,
        plan: Option<Plan>,
    ) -> Self {
        Self {
            id: 0,
            name,
            lastname,
            email,
            phonenumber,
            joined: "".into(),
            plan: plan.unwrap_or_default(),
        }
    }

    pub fn download_all(conn: &mut PooledConn) -> Result<HashMap<u32, User>, Status> {
        Ok(convert!(
            conn.query_map(
                "SELECT id, name, lastname, email, phonenumber, joined, follow, simulate, develop,
                earn from users inner JOIN permissions on users.id = permissions.user_id",
                |(id, name, lastname, email, phonenumber, joined, follow, simulate, develop, earn)| User {
                    id,
                    name,
                    lastname,
                    email,
                    phonenumber,
                    joined,
                    plan: Plan {
                        follow,
                        simulate,
                        develop,
                        earn,
                    },
                },
            ),
            ErrorCode::Database
        )?
        .into_iter()
        .map(|user| (user.id, user))
        .collect())
    }

    // TODO: use plugin.rs as a reference and fix this
    pub fn upload(mut self, conn: &mut PooledConn) -> Result<Self, Status> {
        let mut tx = convert!(
            conn.start_transaction(TxOpts::default()),
            ErrorCode::Database
        )?;
        convert!(
            tx.exec_drop(
                r"INSERT INTO users (name, lastname, email, phonenumber)
              VALUES (:name, :lastname, :email, :number)",
                params! {
                    "name" => &self.name,
                    "lastname" => &self.lastname,
                    "email" => &self.email,
                    "number" => &self.phonenumber,
                },
            ),
            ErrorCode::Database
        )?;
        self.id = convert!(
            tx.query_first(r"SELECT LAST_INSERT_ID()"),
            ErrorCode::Database
        )?
        .unwrap();
        convert!(
            tx.exec_drop(
                r"INSERT INTO permissions (user_id, follow, simulate, develop, earn)
                VALUES (:user_id, :follow, :simulate, :develop, :earn)",
                params! {
                    "user_id" => self.id,
                    "follow" => self.plan.follow,
                    "simulate" => self.plan.simulate,
                    "develop" => self.plan.develop,
                    "earn" => self.plan.earn
                },
            ),
            ErrorCode::Database
        )?;
        convert!(tx.commit(), ErrorCode::Database)?;
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub follow: u8,
    pub simulate: u8,
    pub develop: u8,
    pub earn: u8,
}

impl Default for Plan {
    fn default() -> Self {
        Self { follow: 1, simulate: 1, develop: 1, earn: 0 }
    }
}

mod test {

    #[test]
    fn transfer() {
        use super::*;
        let user = User::new(
            "name".into(),
            Some("lastname".into()),
            "email".into(),
            "phonenumber".into(),
            Some(Plan {
                follow: 1,
                simulate: 2,
                develop: 3,
                earn: 4,
            }),
        );
        let pack = serde_json::to_string(&user).unwrap();
        println!("{}", pack);
    }
}
