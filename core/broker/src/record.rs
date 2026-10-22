use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

/// One call to a brokerage and what came back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub op: String,
    /// What was asked (symbol, order id, client id...). Replay checks it.
    pub key: String,
    pub result: std::result::Result<Value, BrokerError>,
}

/// Where recorded calls go. The deployment appends them to its event log.
pub type Sink = Arc<Mutex<dyn FnMut(&Call) + Send>>;

pub struct Recording<B> {
    inner: B,
    sink: Sink,
}

impl<B: Broker> Recording<B> {
    pub fn new(inner: B, sink: Sink) -> Self {
        Self { inner, sink }
    }

    fn rec<T: Serialize>(&self, op: &str, key: &str, r: &Result<T>) {
        let result = match r {
            Ok(v) => Ok(serde_json::to_value(v).unwrap_or(Value::Null)),
            Err(e) => Err(e.clone()),
        };
        if let Ok(mut f) = self.sink.lock() {
            f(&Call { op: op.into(), key: key.into(), result });
        }
    }
}

macro_rules! recorded {
    ($self:ident, $op:literal, $key:expr, $call:expr) => {{
        let r = $call;
        $self.rec($op, $key, &r);
        r
    }};
}

impl<B: Broker> Broker for Recording<B> {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn environment(&self) -> Environment {
        self.inner.environment()
    }
    fn account(&mut self) -> Result<Account> {
        recorded!(self, "account", "", self.inner.account())
    }
    fn positions(&mut self) -> Result<Vec<Position>> {
        recorded!(self, "positions", "", self.inner.positions())
    }
    fn quote(&mut self, symbol: &str) -> Result<Quote> {
        recorded!(self, "quote", symbol, self.inner.quote(symbol))
    }
    fn place_order(&mut self, req: &OrderRequest) -> Result<Order> {
        recorded!(self, "place_order", &req.client_id, self.inner.place_order(req))
    }
    fn cancel_order(&mut self, id: &str) -> Result<Order> {
        recorded!(self, "cancel_order", id, self.inner.cancel_order(id))
    }
    fn order(&mut self, id: &str) -> Result<Order> {
        recorded!(self, "order", id, self.inner.order(id))
    }
    fn open_orders(&mut self) -> Result<Vec<Order>> {
        recorded!(self, "open_orders", "", self.inner.open_orders())
    }
    fn on_bar(&mut self, c: &BTreeMap<String, Candle>) {
        self.inner.on_bar(c)
    }
    fn drain_notes(&mut self) -> Vec<String> {
        self.inner.drain_notes()
    }
}

/// Serves recorded calls back, in order, while a strategy is re-run from its
/// bar history after a restart; once they run out it talks to the real broker.
/// If the strategy now asks for something different from what was recorded the
/// run has diverged and the call fails instead of guessing.
pub struct Replay<B> {
    inner: B,
    queue: VecDeque<Call>,
}

impl<B: Broker> Replay<B> {
    pub fn new(inner: B, calls: Vec<Call>) -> Self {
        Self { inner, queue: calls.into() }
    }

    pub fn remaining(&self) -> usize {
        self.queue.len()
    }

    fn next<T: for<'de> Deserialize<'de>>(&mut self, op: &str, key: &str) -> Option<Result<T>> {
        let head = self.queue.front()?;
        if head.op != op || head.key != key {
            let msg = format!("restart recovery diverged: expected `{op} {key}` but the log has `{} {}`", head.op, head.key);
            self.queue.clear();
            return Some(Err(BrokerError::new(ErrorKind::Diverged, msg)));
        }
        let call = self.queue.pop_front()?;
        Some(match call.result {
            Ok(v) => serde_json::from_value(v).map_err(|e| BrokerError::new(ErrorKind::Diverged, format!("unreadable log entry: {e}"))),
            Err(e) => Err(e),
        })
    }
}

macro_rules! replayed {
    ($self:ident, $op:literal, $key:expr, $call:expr) => {
        match $self.next($op, $key) {
            Some(r) => r,
            None => $call,
        }
    };
}

impl<B: Broker> Broker for Replay<B> {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn environment(&self) -> Environment {
        self.inner.environment()
    }
    fn account(&mut self) -> Result<Account> {
        replayed!(self, "account", "", self.inner.account())
    }
    fn positions(&mut self) -> Result<Vec<Position>> {
        replayed!(self, "positions", "", self.inner.positions())
    }
    fn quote(&mut self, symbol: &str) -> Result<Quote> {
        replayed!(self, "quote", symbol, self.inner.quote(symbol))
    }
    fn place_order(&mut self, req: &OrderRequest) -> Result<Order> {
        replayed!(self, "place_order", &req.client_id, self.inner.place_order(req))
    }
    fn cancel_order(&mut self, id: &str) -> Result<Order> {
        replayed!(self, "cancel_order", id, self.inner.cancel_order(id))
    }
    fn order(&mut self, id: &str) -> Result<Order> {
        replayed!(self, "order", id, self.inner.order(id))
    }
    fn open_orders(&mut self) -> Result<Vec<Order>> {
        replayed!(self, "open_orders", "", self.inner.open_orders())
    }
    fn on_bar(&mut self, c: &BTreeMap<String, Candle>) {
        self.inner.on_bar(c)
    }
    fn drain_notes(&mut self) -> Vec<String> {
        self.inner.drain_notes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str) -> OrderRequest {
        OrderRequest { symbol: "X".into(), side: Side::Buy, qty: 1.0, kind: Kind::Market, tif: Tif::Day, client_id: id.into() }
    }

    fn paper() -> PaperBroker {
        let mut p = PaperBroker::new(1000.0, 0.0);
        p.set_price("X", 10.0);
        p
    }

    #[test]
    fn recorded_calls_replay_without_touching_the_broker_then_hand_over() {
        let log: Arc<Mutex<Vec<Call>>> = Arc::default();
        let l2 = log.clone();
        let sink: Sink = Arc::new(Mutex::new(move |c: &Call| l2.lock().unwrap().push(c.clone())));
        let mut rec = Recording::new(paper(), sink);
        rec.quote("X").unwrap();
        let first = rec.place_order(&req("a")).unwrap();
        assert!(rec.place_order(&OrderRequest { qty: -1.0, ..req("bad") }).is_err());
        let calls = log.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);

        // a fresh broker that has never seen the order: replay must not ask it
        let mut rep = Replay::new(paper(), calls);
        assert_eq!(rep.quote("X").unwrap().last, 10.0);
        assert_eq!(rep.place_order(&req("a")).unwrap(), first);
        assert!(rep.place_order(&OrderRequest { qty: -1.0, ..req("bad") }).is_err());
        assert_eq!(rep.remaining(), 0);
        assert!(rep.positions().unwrap().is_empty(), "the inner broker never saw the replayed order");
        assert_eq!(rep.place_order(&req("b")).unwrap().id, "P1");
    }

    #[test]
    fn a_different_request_than_recorded_is_a_divergence() {
        let log: Arc<Mutex<Vec<Call>>> = Arc::default();
        let l2 = log.clone();
        let sink: Sink = Arc::new(Mutex::new(move |c: &Call| l2.lock().unwrap().push(c.clone())));
        let mut rec = Recording::new(paper(), sink);
        rec.place_order(&req("a")).unwrap();
        let mut rep = Replay::new(paper(), log.lock().unwrap().clone());
        let e = rep.place_order(&req("different")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Diverged);
    }
}
