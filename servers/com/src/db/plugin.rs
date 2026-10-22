use crate::{
    convert,
    data::Contents,
    status::{ErrorCode, Status}, failed, net::{PluginId, UserId, PluginName},
};
use chrono::NaiveDate;
use mysql::{params, prelude::Queryable, PooledConn, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct Plugin {
    pub id: u32,
    pub user_id: u32,
    pub name: String,
    pub source: String,
    pub lib: Option<Vec<u8>>,
    pub version: String,
    pub price: Option<f32>,
    pub machines: Option<Contents>,
    pub date: Option<NaiveDate>,
    pub published: bool,
}

impl Plugin {
    pub fn new(
        txn: &mut Transaction,
        user_id: u32,
        name: String,
        source: String,
        version: String,
    ) -> Result<Self, Status> {
        let mut res = Self {
            id: 0,
            user_id,
            name,
            source,
            lib: None,
            version,
            price: None,
            date: None,
            machines: Default::default(),
            published: false,
        };

        convert!(txn.exec_drop(
            r"INSERT INTO plugins (user_id, name, source, machines, lib, version, price, published)
              VALUES (:user_id, :name, :source, :machines, :lib, :version, :price, :published)",
            params! {
                "user_id" => res.user_id,
                "name" => &res.name,
                "source" => &res.source,
                "machines" => res.machines.clone().map(|m| m.into_json().unwrap()),
                "lib" => &res.lib,
                "version" => &res.version,
                "price" => &res.price,
                "published" => &res.published
            },
        ), ErrorCode::Database)?;
        let val: Option<u32> = convert!(txn.query_first("SELECT LAST_INSERT_ID()"))?;
        if let Some(id) = val {
            res.id = id;
            Ok(res)
        } else {
            Err(failed!("Could not insert into database", ErrorCode::Database))
        }
    }

    pub fn download_all(conn: &mut PooledConn) -> Result<HashMap<u32, Plugin>, Status> {
        Ok(convert!(
            conn.query_map(
                "SELECT * FROM plugins",
                |(id, user_id, name, source, machines, lib, version, price, date, published)| {
                    let machines: Option<String> = machines;
                    (
                        id,
                        Plugin {
                            id,
                            user_id,
                            name,
                            source,
                            lib,
                            version,
                            price,
                            date,
                            machines: machines.map(|m| Contents::from_json(m.as_str())
                                .expect("Invalid format for 'machines'")),
                            published,
                        },
                    )
                },
            ),
            ErrorCode::Database
        )?
        .into_iter()
        .collect())
    }


    pub fn by_id(txn: &mut Transaction, id: PluginId) -> Result<Plugin, Status> {
        let mut res = convert!(
            txn.query_map(
                format!("SELECT * FROM plugins WHERE id={id}"),
                |(id, user_id, name, source, machines, lib, version, price, date, published)| {
                    let machines: Option<String> = machines;
                    (
                        id,
                        Plugin {
                            id,
                            user_id,
                            name,
                            source,
                            lib,
                            version,
                            price,
                            date,
                            machines: machines.map(|m| Contents::from_json(m.as_str())
                                .expect("Invalid format for 'machines'")),
                            published,
                        },
                    )
                },
            ),
            ErrorCode::Database
        )?;
        if let Some((_, plugin)) = res.pop() {
            Ok(plugin)
        } else {
            Err(failed!(format!("No plugin with id {id}")))
        }
    }

    pub fn by_user(txn: &mut Transaction, user: UserId, name: PluginName) -> Result<Plugin, Status> {
        let mut res = convert!(
            txn.query_map(
                format!("SELECT * FROM plugins WHERE user_id={user} AND name='{name}'"),
                |(id, user_id, name, source, machines, lib, version, price, date, published)| {
                    let machines: Option<String> = machines;
                    (
                        id,
                        Plugin {
                            id,
                            user_id,
                            name,
                            source,
                            lib,
                            version,
                            price,
                            date,
                            machines: machines.map(|m| Contents::from_json(m.as_str())
                                .expect("Invalid format for 'machines'")),
                            published,
                        },
                    )
                },
            ),
            ErrorCode::Database
        )?;
        if let Some((_, plugin)) = res.pop() {
            Ok(plugin)
        } else {
            Err(failed!(format!("No plugin '{name}' for user {user}")))
        }
    }

    pub fn upload(self, txn: &mut Transaction) -> Result<Self, Status> {
        convert!(txn.exec_drop(
            // not REPLACE: that deletes the row first, which would also delete the bots and
            // subscriptions that refer to the plugin
            r"INSERT INTO plugins (id, user_id, name, source, machines, lib, version, price, date, published)
              VALUES (:id, :user_id, :name, :source, :machines, :lib, :version, :price, :date, :published)
              ON DUPLICATE KEY UPDATE user_id = VALUES(user_id), name = VALUES(name), source = VALUES(source),
                  machines = VALUES(machines), lib = VALUES(lib), version = VALUES(version), price = VALUES(price),
                  date = VALUES(date), published = VALUES(published)",
            params! {
                "id" => self.id,
                "user_id" => self.user_id,
                "name" => &self.name,
                "source" => &self.source,
                "machines" => self.machines.clone().map(|m| m.into_json().unwrap()),
                "lib" => &self.lib,
                "version" => &self.version,
                "price" => self.price,
                "date" => self.date,
                "published" => self.published
            },
        ), ErrorCode::Database)?;
        Ok(self)
    }
}
