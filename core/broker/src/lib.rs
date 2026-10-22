//! A tiny brokerage abstraction.
//!
//! * [`Broker`] is the one trait the rest of the platform talks to.
//! * [`PaperBroker`] is a deterministic simulated account used for backtests and paper trading.
//! * [`RestBroker`] drives any HTTP brokerage from a JSON description (see `docs/brokers.md`).
//! * [`Guarded`] wraps any broker with hard limits and a kill switch.
//! * [`Recording`] / [`Replay`] make a broker-backed run recoverable after a restart.

pub mod fake;
mod fills;
mod guard;
mod paper;
mod record;
mod rest;
mod types;

pub use fills::{Fill, FillTracker};
pub use guard::{Guarded, KillSwitch, Limits};
pub use paper::PaperBroker;
pub use record::{Call, Recording, Replay, Sink};
pub use rest::{RestBroker, Spec};
pub use types::*;

use std::collections::BTreeMap;

/// Everything the platform needs from a brokerage. Implementations are blocking:
/// each deployment runs on its own thread.
pub trait Broker: Send {
    fn name(&self) -> &str;
    fn environment(&self) -> Environment;

    fn account(&mut self) -> Result<Account>;
    fn positions(&mut self) -> Result<Vec<Position>>;
    fn quote(&mut self, symbol: &str) -> Result<Quote>;
    fn place_order(&mut self, req: &OrderRequest) -> Result<Order>;
    fn cancel_order(&mut self, id: &str) -> Result<Order>;
    fn order(&mut self, id: &str) -> Result<Order>;
    fn open_orders(&mut self) -> Result<Vec<Order>>;

    /// A new bar closed. Simulated brokers use it to work resting orders;
    /// real brokerages ignore it.
    fn on_bar(&mut self, _candles: &BTreeMap<String, Candle>) {}

    /// Human-readable notes about things the wrapper did (reduced, blocked...).
    fn drain_notes(&mut self) -> Vec<String> {
        Vec::new()
    }
}

impl Broker for Box<dyn Broker> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn environment(&self) -> Environment {
        (**self).environment()
    }
    fn account(&mut self) -> Result<Account> {
        (**self).account()
    }
    fn positions(&mut self) -> Result<Vec<Position>> {
        (**self).positions()
    }
    fn quote(&mut self, symbol: &str) -> Result<Quote> {
        (**self).quote(symbol)
    }
    fn place_order(&mut self, req: &OrderRequest) -> Result<Order> {
        (**self).place_order(req)
    }
    fn cancel_order(&mut self, id: &str) -> Result<Order> {
        (**self).cancel_order(id)
    }
    fn order(&mut self, id: &str) -> Result<Order> {
        (**self).order(id)
    }
    fn open_orders(&mut self) -> Result<Vec<Order>> {
        (**self).open_orders()
    }
    fn on_bar(&mut self, candles: &BTreeMap<String, Candle>) {
        (**self).on_bar(candles)
    }
    fn drain_notes(&mut self) -> Vec<String> {
        (**self).drain_notes()
    }
}
