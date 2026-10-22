use crate::*;
use std::collections::{BTreeMap, HashMap};

/// A deterministic simulated account.
///
/// * market orders fill at the last close (so an order placed while a bar is
///   being processed fills at that bar's close);
/// * limit and stop orders rest until a later bar trades through them, and fill
///   at the limit/stop price or at the open when the bar gapped past it;
/// * fees are a rate of the traded value; there is no shorting and no margin;
/// * "day" orders are treated like good-till-cancelled (bars have no session).
pub struct PaperBroker {
    name: String,
    cash: f64,
    fee: f64,
    last: BTreeMap<String, f64>,
    positions: BTreeMap<String, (f64, f64)>, // qty, average cost
    orders: Vec<Order>,
    by_client: HashMap<String, usize>,
    next: u64,
    environment: Environment,
}

const EPS: f64 = 1e-9;

impl PaperBroker {
    pub fn new(initial_cash: f64, fee_bps: f64) -> Self {
        Self {
            name: "paper".into(),
            cash: initial_cash,
            fee: fee_bps / 10_000.0,
            last: BTreeMap::new(),
            positions: BTreeMap::new(),
            orders: Vec::new(),
            by_client: HashMap::new(),
            next: 1,
            environment: Environment::Simulated,
        }
    }

    pub fn named(mut self, name: &str, environment: Environment) -> Self {
        self.name = name.to_owned();
        self.environment = environment;
        self
    }

    pub fn set_price(&mut self, symbol: &str, price: f64) {
        self.last.insert(symbol.to_owned(), price);
    }

    /// Execute up to `want` shares of order `idx` at `price`.
    fn execute(&mut self, idx: usize, price: f64) {
        let (symbol, side, want) = {
            let o = &self.orders[idx];
            (o.symbol.clone(), o.side, o.qty - o.filled_qty)
        };
        let (qty, note) = match side {
            Side::Buy => {
                let q = want.min(self.cash / (price * (1.0 + self.fee)));
                (q.max(0.0), "not enough cash")
            }
            Side::Sell => {
                let held = self.positions.get(&symbol).map_or(0.0, |p| p.0);
                (want.min(held), "no position to sell")
            }
        };
        let o = &mut self.orders[idx];
        if qty <= EPS {
            o.state = OrderState::Rejected;
            o.message = note.into();
            return;
        }
        let fee = price * qty * self.fee;
        let pos = self.positions.entry(symbol).or_insert((0.0, 0.0));
        match side {
            Side::Buy => {
                pos.1 = (pos.1 * pos.0 + price * qty) / (pos.0 + qty);
                pos.0 += qty;
                self.cash -= price * qty + fee;
            }
            Side::Sell => {
                pos.0 -= qty;
                if pos.0 < 1e-12 {
                    *pos = (0.0, 0.0);
                }
                self.cash += price * qty - fee;
            }
        }
        o.avg_price = if o.filled_qty <= 0.0 { price } else { (o.avg_price * o.filled_qty + price * qty) / (o.filled_qty + qty) };
        o.filled_qty += qty;
        o.fees += fee;
        if o.filled_qty >= o.qty - EPS {
            o.state = OrderState::Filled;
        } else {
            o.state = OrderState::Canceled;
            o.message = format!("reduced to {} shares: {note}", o.filled_qty);
        }
    }

    fn marketable(kind: Kind, side: Side, last: f64) -> bool {
        match (kind, side) {
            (Kind::Market, _) => true,
            (Kind::Limit { limit }, Side::Buy) => last <= limit,
            (Kind::Limit { limit }, Side::Sell) => last >= limit,
            (Kind::Stop { stop }, Side::Buy) => last >= stop,
            (Kind::Stop { stop }, Side::Sell) => last <= stop,
        }
    }
}

impl Broker for PaperBroker {
    fn name(&self) -> &str {
        &self.name
    }

    fn environment(&self) -> Environment {
        self.environment
    }

    fn account(&mut self) -> Result<Account> {
        let held: f64 = self.positions.iter().map(|(s, p)| p.0 * self.last.get(s).copied().unwrap_or(p.1)).sum();
        Ok(Account { cash: self.cash, equity: self.cash + held, buying_power: self.cash, currency: "USD".into() })
    }

    fn positions(&mut self) -> Result<Vec<Position>> {
        Ok(self
            .positions
            .iter()
            .filter(|(_, p)| p.0 > 1e-12)
            .map(|(s, p)| Position {
                symbol: s.clone(),
                qty: p.0,
                avg_cost: p.1,
                market_value: p.0 * self.last.get(s).copied().unwrap_or(p.1),
            })
            .collect())
    }

    fn quote(&mut self, symbol: &str) -> Result<Quote> {
        match self.last.get(symbol) {
            Some(&p) => Ok(Quote { symbol: symbol.into(), bid: p, ask: p, last: p }),
            None => Err(BrokerError::rejected(format!("no price for `{symbol}`"))),
        }
    }

    fn place_order(&mut self, req: &OrderRequest) -> Result<Order> {
        if let Some(&i) = self.by_client.get(&req.client_id) {
            return Ok(self.orders[i].clone());
        }
        if !(req.qty.is_finite() && req.qty > 0.0) {
            return Err(BrokerError::rejected(format!("invalid quantity {}", req.qty)));
        }
        if let Kind::Limit { limit: p } | Kind::Stop { stop: p } = req.kind {
            if !(p.is_finite() && p > 0.0) {
                return Err(BrokerError::rejected(format!("invalid price {p}")));
            }
        }
        let Some(&last) = self.last.get(&req.symbol) else {
            return Err(BrokerError::rejected(format!("no price for `{}`", req.symbol)));
        };
        let id = format!("P{}", self.next);
        self.next += 1;
        self.orders.push(Order {
            id,
            client_id: req.client_id.clone(),
            symbol: req.symbol.clone(),
            side: req.side,
            qty: req.qty,
            kind: req.kind,
            tif: req.tif,
            state: OrderState::Open,
            filled_qty: 0.0,
            avg_price: 0.0,
            fees: 0.0,
            message: String::new(),
        });
        let idx = self.orders.len() - 1;
        self.by_client.insert(req.client_id.clone(), idx);
        if Self::marketable(req.kind, req.side, last) {
            self.execute(idx, last);
        }
        Ok(self.orders[idx].clone())
    }

    fn cancel_order(&mut self, id: &str) -> Result<Order> {
        let Some(o) = self.orders.iter_mut().find(|o| o.id == id) else {
            return Err(BrokerError::rejected(format!("unknown order `{id}`")));
        };
        if !o.state.is_done() {
            o.state = OrderState::Canceled;
        }
        Ok(o.clone())
    }

    fn order(&mut self, id: &str) -> Result<Order> {
        self.orders.iter().find(|o| o.id == id).cloned().ok_or_else(|| BrokerError::rejected(format!("unknown order `{id}`")))
    }

    fn open_orders(&mut self) -> Result<Vec<Order>> {
        Ok(self.orders.iter().filter(|o| !o.state.is_done()).cloned().collect())
    }

    fn on_bar(&mut self, candles: &BTreeMap<String, Candle>) {
        for idx in 0..self.orders.len() {
            if self.orders[idx].state.is_done() {
                continue;
            }
            let o = &self.orders[idx];
            let Some(c) = candles.get(&o.symbol) else { continue };
            let price = match (o.kind, o.side) {
                (Kind::Limit { limit }, Side::Buy) if c.low <= limit => Some(limit.min(c.open)),
                (Kind::Limit { limit }, Side::Sell) if c.high >= limit => Some(limit.max(c.open)),
                (Kind::Stop { stop }, Side::Buy) if c.high >= stop => Some(stop.max(c.open)),
                (Kind::Stop { stop }, Side::Sell) if c.low <= stop => Some(stop.min(c.open)),
                _ => None,
            };
            if let Some(p) = price {
                self.execute(idx, p);
            }
        }
        for (s, c) in candles {
            self.last.insert(s.clone(), c.close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(side: Side, qty: f64, kind: Kind, id: &str) -> OrderRequest {
        OrderRequest { symbol: "X".into(), side, qty, kind, tif: Tif::Gtc, client_id: id.into() }
    }

    fn candle(o: f64, h: f64, l: f64, c: f64) -> BTreeMap<String, Candle> {
        BTreeMap::from([("X".to_owned(), Candle { open: o, high: h, low: l, close: c })])
    }

    fn broker() -> PaperBroker {
        let mut b = PaperBroker::new(1000.0, 0.0);
        b.set_price("X", 10.0);
        b
    }

    #[test]
    fn market_orders_fill_at_the_last_price_with_fees() {
        let mut b = PaperBroker::new(1000.0, 100.0); // 1%
        b.set_price("X", 10.0);
        let o = b.place_order(&req(Side::Buy, 50.0, Kind::Market, "a")).unwrap();
        assert_eq!((o.state, o.filled_qty, o.avg_price), (OrderState::Filled, 50.0, 10.0));
        assert!((o.fees - 5.0).abs() < 1e-9);
        assert!((b.account().unwrap().cash - 495.0).abs() < 1e-9);
        let o = b.place_order(&req(Side::Sell, 50.0, Kind::Market, "b")).unwrap();
        assert_eq!(o.state, OrderState::Filled);
        assert!((b.account().unwrap().cash - 990.0).abs() < 1e-9);
    }

    #[test]
    fn repeated_client_ids_are_the_same_order() {
        let mut b = broker();
        let a = b.place_order(&req(Side::Buy, 10.0, Kind::Market, "same")).unwrap();
        let c = b.place_order(&req(Side::Buy, 10.0, Kind::Market, "same")).unwrap();
        assert_eq!(a.id, c.id);
        assert_eq!(b.positions().unwrap()[0].qty, 10.0);
    }

    #[test]
    fn limit_orders_rest_until_the_price_trades_through() {
        let mut b = broker();
        let o = b.place_order(&req(Side::Buy, 10.0, Kind::Limit { limit: 9.0 }, "l")).unwrap();
        assert_eq!(o.state, OrderState::Open);
        b.on_bar(&candle(10.0, 10.5, 9.5, 10.2));
        assert_eq!(b.order(&o.id).unwrap().state, OrderState::Open);
        b.on_bar(&candle(9.8, 9.9, 8.8, 9.2));
        let o = b.order(&o.id).unwrap();
        assert_eq!((o.state, o.avg_price), (OrderState::Filled, 9.0));
    }

    #[test]
    fn a_gap_fills_at_the_open_not_the_limit() {
        let mut b = broker();
        let o = b.place_order(&req(Side::Buy, 10.0, Kind::Limit { limit: 9.0 }, "l")).unwrap();
        b.on_bar(&candle(8.0, 8.5, 7.9, 8.2));
        assert_eq!(b.order(&o.id).unwrap().avg_price, 8.0);
        // a stop sell that gaps down fills at the open too
        b.place_order(&req(Side::Sell, 10.0, Kind::Stop { stop: 8.0 }, "s")).unwrap();
        b.on_bar(&candle(7.0, 7.5, 6.5, 7.2));
        let s = b.order("P2").unwrap();
        assert_eq!((s.state, s.avg_price), (OrderState::Filled, 7.0));
    }

    #[test]
    fn marketable_limits_fill_immediately_and_cancel_works() {
        let mut b = broker();
        let o = b.place_order(&req(Side::Buy, 5.0, Kind::Limit { limit: 11.0 }, "m")).unwrap();
        assert_eq!((o.state, o.avg_price), (OrderState::Filled, 10.0));
        let r = b.place_order(&req(Side::Buy, 5.0, Kind::Limit { limit: 5.0 }, "r")).unwrap();
        assert_eq!(b.open_orders().unwrap().len(), 1);
        assert_eq!(b.cancel_order(&r.id).unwrap().state, OrderState::Canceled);
        assert!(b.open_orders().unwrap().is_empty());
        b.on_bar(&candle(5.0, 5.0, 4.0, 4.5));
        assert_eq!(b.order(&r.id).unwrap().filled_qty, 0.0);
    }

    #[test]
    fn cash_and_position_limits_reduce_or_reject() {
        let mut b = broker();
        let o = b.place_order(&req(Side::Buy, 500.0, Kind::Market, "big")).unwrap();
        assert_eq!((o.state, o.filled_qty), (OrderState::Canceled, 100.0));
        assert!(o.message.contains("not enough cash"));
        let o = b.place_order(&req(Side::Sell, 300.0, Kind::Market, "s")).unwrap();
        assert_eq!(o.filled_qty, 100.0);
        let o = b.place_order(&req(Side::Sell, 1.0, Kind::Market, "s2")).unwrap();
        assert_eq!(o.state, OrderState::Rejected);
    }

    #[test]
    fn bad_requests_are_rejected() {
        let mut b = broker();
        assert!(b.place_order(&req(Side::Buy, 0.0, Kind::Market, "z")).is_err());
        assert!(b.place_order(&req(Side::Buy, f64::NAN, Kind::Market, "n")).is_err());
        assert!(b.place_order(&req(Side::Buy, 1.0, Kind::Limit { limit: -1.0 }, "p")).is_err());
        let mut other = req(Side::Buy, 1.0, Kind::Market, "u");
        other.symbol = "NOPE".into();
        assert!(b.place_order(&other).is_err());
    }

    #[test]
    fn equity_marks_positions_to_the_last_price() {
        let mut b = broker();
        b.place_order(&req(Side::Buy, 10.0, Kind::Market, "a")).unwrap();
        b.on_bar(&candle(10.0, 12.0, 10.0, 12.0));
        let a = b.account().unwrap();
        assert_eq!((a.cash, a.equity), (900.0, 1020.0));
        assert_eq!(b.positions().unwrap()[0].market_value, 120.0);
    }
}
