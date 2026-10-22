use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A shared emergency stop. While tripped no *buy* order can be placed; selling
/// and cancelling still work, so positions and working orders can always be closed.
#[derive(Clone, Default)]
pub struct KillSwitch(Arc<AtomicBool>);

impl KillSwitch {
    pub fn trip(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
    pub fn is_tripped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Limits enforced *outside* the strategy: no strategy bug can get past them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Largest single order, in account currency.
    pub max_order_value: Option<f64>,
    /// Largest position after an order fills.
    pub max_position_value: Option<f64>,
    /// Most orders this deployment may place.
    pub max_orders: Option<u32>,
    /// If set, only these symbols may be traded.
    pub symbols: Option<Vec<String>>,
}

impl Limits {
    /// Real-money connections must have both value limits.
    pub fn complete_for_live(&self) -> std::result::Result<(), String> {
        if self.max_order_value.is_none() || self.max_position_value.is_none() {
            return Err("a live connection needs both `max_order_value` and `max_position_value`".into());
        }
        Ok(())
    }
}

pub struct Guarded<B> {
    inner: B,
    limits: Limits,
    kill: KillSwitch,
    placed: u32,
    notes: Vec<String>,
}

impl<B: Broker> Guarded<B> {
    pub fn new(inner: B, limits: Limits, kill: KillSwitch) -> std::result::Result<Self, String> {
        if inner.environment() == Environment::Live {
            limits.complete_for_live()?;
        }
        Ok(Self { inner, limits, kill, placed: 0, notes: Vec::new() })
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn set_limits(&mut self, limits: Limits) -> std::result::Result<(), String> {
        if self.inner.environment() == Environment::Live {
            limits.complete_for_live()?;
        }
        self.limits = limits;
        Ok(())
    }

    fn block<T>(&mut self, why: String) -> Result<T> {
        self.notes.push(format!("blocked: {why}"));
        Err(BrokerError::blocked(why))
    }

    fn check(&mut self, req: &OrderRequest) -> Result<()> {
        // Only orders that add exposure are stopped: closing positions must stay possible.
        if self.kill.is_tripped() && req.side == Side::Buy {
            return self.block("the kill switch is on (buying is stopped; selling and cancelling still work)".into());
        }
        if let Some(allowed) = &self.limits.symbols {
            if !allowed.iter().any(|s| s == &req.symbol) {
                return self.block(format!("`{}` is not in the allowed symbols", req.symbol));
            }
        }
        if let Some(max) = self.limits.max_orders {
            if self.placed >= max {
                return self.block(format!("order limit of {max} reached"));
            }
        }
        let price = match req.kind {
            Kind::Limit { limit } => limit,
            Kind::Stop { stop } => stop,
            Kind::Market => match self.inner.quote(&req.symbol) {
                Ok(q) => q.ask.max(q.last),
                Err(e) => return self.block(format!("cannot price the order ({e})")),
            },
        };
        let value = req.qty * price;
        if let Some(max) = self.limits.max_order_value {
            if value > max {
                return self.block(format!("order value {value:.2} is above the limit {max:.2}"));
            }
        }
        if let (Some(max), Side::Buy) = (self.limits.max_position_value, req.side) {
            let held = match self.inner.positions() {
                Ok(p) => p.iter().find(|p| p.symbol == req.symbol).map_or(0.0, |p| p.qty * price),
                Err(e) => return self.block(format!("cannot read positions ({e})")),
            };
            if held + value > max {
                return self.block(format!("position would be {:.2}, above the limit {max:.2}", held + value));
            }
        }
        Ok(())
    }
}

impl<B: Broker> Broker for Guarded<B> {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn environment(&self) -> Environment {
        self.inner.environment()
    }
    fn account(&mut self) -> Result<Account> {
        self.inner.account()
    }
    fn positions(&mut self) -> Result<Vec<Position>> {
        self.inner.positions()
    }
    fn quote(&mut self, symbol: &str) -> Result<Quote> {
        self.inner.quote(symbol)
    }
    fn place_order(&mut self, req: &OrderRequest) -> Result<Order> {
        self.check(req)?;
        self.placed += 1;
        self.inner.place_order(req)
    }
    fn cancel_order(&mut self, id: &str) -> Result<Order> {
        self.inner.cancel_order(id)
    }
    fn order(&mut self, id: &str) -> Result<Order> {
        self.inner.order(id)
    }
    fn open_orders(&mut self) -> Result<Vec<Order>> {
        self.inner.open_orders()
    }
    fn on_bar(&mut self, c: &BTreeMap<String, Candle>) {
        self.inner.on_bar(c)
    }
    fn drain_notes(&mut self) -> Vec<String> {
        let mut n = std::mem::take(&mut self.notes);
        n.extend(self.inner.drain_notes());
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(side: Side, sym: &str, qty: f64, id: &str) -> OrderRequest {
        OrderRequest { symbol: sym.into(), side, qty, kind: Kind::Market, tif: Tif::Day, client_id: id.into() }
    }

    fn paper() -> PaperBroker {
        let mut p = PaperBroker::new(100_000.0, 0.0);
        p.set_price("A", 10.0);
        p.set_price("B", 10.0);
        p
    }

    #[test]
    fn order_value_position_symbol_and_count_limits() {
        let limits = Limits { max_order_value: Some(500.0), max_position_value: Some(800.0), max_orders: Some(4), symbols: Some(vec!["A".into()]) };
        let mut g = Guarded::new(paper(), limits, KillSwitch::default()).unwrap();
        assert!(g.place_order(&req(Side::Buy, "A", 60.0, "1")).is_err()); // 600 > 500
        assert!(g.place_order(&req(Side::Buy, "A", 50.0, "2")).is_ok());
        assert!(g.place_order(&req(Side::Buy, "A", 30.0, "3")).is_ok()); // position 800
        let e = g.place_order(&req(Side::Buy, "A", 1.0, "4")).unwrap_err(); // 810 > 800
        assert_eq!(e.kind, ErrorKind::Blocked);
        assert!(g.place_order(&req(Side::Buy, "B", 1.0, "5")).is_err());
        assert!(g.place_order(&req(Side::Sell, "A", 10.0, "6")).is_ok());
        assert!(g.place_order(&req(Side::Sell, "A", 10.0, "7")).is_ok());
        assert!(g.place_order(&req(Side::Sell, "A", 10.0, "8")).is_err()); // count
        assert!(g.drain_notes().iter().all(|n| n.starts_with("blocked: ")));
    }

    #[test]
    fn kill_switch_blocks_buying_but_not_selling_or_cancelling() {
        let kill = KillSwitch::default();
        let mut g = Guarded::new(paper(), Limits::default(), kill.clone()).unwrap();
        let o = g.place_order(&OrderRequest { kind: Kind::Limit { limit: 1.0 }, ..req(Side::Buy, "A", 1.0, "1") }).unwrap();
        g.place_order(&req(Side::Buy, "A", 5.0, "held")).unwrap();
        kill.trip();
        assert!(g.place_order(&req(Side::Buy, "A", 1.0, "2")).is_err());
        assert!(g.place_order(&req(Side::Sell, "A", 5.0, "sell")).is_ok());
        assert_eq!(g.cancel_order(&o.id).unwrap().state, OrderState::Canceled);
        kill.reset();
        assert!(g.place_order(&req(Side::Buy, "A", 1.0, "3")).is_ok());
    }

    #[test]
    fn live_connections_require_both_value_limits() {
        let live = paper().named("real", Environment::Live);
        assert!(Guarded::new(live, Limits { max_order_value: Some(1.0), ..Limits::default() }, KillSwitch::default()).is_err());
        let live = paper().named("real", Environment::Live);
        let lim = Limits { max_order_value: Some(1.0), max_position_value: Some(2.0), ..Limits::default() };
        assert!(Guarded::new(live, lim, KillSwitch::default()).is_ok());
    }
}
