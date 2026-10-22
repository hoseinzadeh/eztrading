mod plugin;
mod user;
use mysql::{Pool, PooledConn, Transaction, TxOpts};
pub use plugin::*;
pub use user::*;

use crate::{
    convert,
    status::{ErrorCode, Status},
};

pub trait TxnError {
    fn is_successful(&self) -> bool;
    fn is_failed(&self) -> bool {
        !self.is_successful()
    }
}

pub trait Error {
    fn err<E: ToString>(e: E) -> Self;
}

pub trait Database {
    fn db_conn(&mut self) -> &mut Option<PooledConn>;
    fn db_conn_str(&self) -> &str;

    fn new_pool(url: &str) -> Result<PooledConn, Status> {
        let pool = convert!(Pool::new(url), ErrorCode::Database)?;
        convert!(pool.get_conn(), ErrorCode::Database)
    }

    fn db_connect(&mut self) -> Result<(), Status>
    where
        Self: Sized,
    {
        *self.db_conn() = Some(Self::new_pool(self.db_conn_str())?);
        Ok(())
    }

    fn db_reconnect(&mut self)
    where
        Self: Sized,
    {
        match self.db_connect() {
            Ok(()) => (),
            Err(e) => eprintln!("Reconnection failed: {e}"),
        }
    }

    // Try reconnecting if error and returning the error
    fn handle_db_error<T, E>(&mut self, r: mysql::Result<Result<T, E>>) -> Result<T, E>
    where
        Self: Sized,
        E: Error,
    {
        if let Err(e) = &r {
            eprintln!("{e} reconnecting...");
            self.db_reconnect()
        }
        r.map_err(|e| E::err(e))?
    }

    fn txn<Rep, E, F: FnOnce(&mut Transaction) -> Result<Rep, E>>(&mut self, f: F) -> Result<Rep, E>
    where
        Rep: TxnError,
        E: Error + From<String>,
        Self: Sized,
    {
        if let Some(conn) = self.db_conn().as_mut() {
            let res = match conn.start_transaction(TxOpts::default()) {
                Ok(mut txn) => match f(&mut txn) {
                    Ok(rep) => {
                        if rep.is_successful() {
                            txn.commit().map(|_| Ok(rep))
                        } else {
                            txn.rollback().map(|_| Ok(rep))
                        }
                    }
                    Err(e) => txn.rollback().map(|_| Err(e)),
                },
                Err(e) => {
                    eprintln!("{e}");
                    Err(e)
                }
            };
            self.handle_db_error(res)
        } else {
            Err(E::from("No DB Connection".to_owned()))
        }
    }
}
