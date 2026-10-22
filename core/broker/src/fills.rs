use crate::{Order, Side};
use std::collections::HashMap;

/// An increment of execution: what happened since the order was last looked at.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    pub price: f64,
    pub fee: f64,
}

/// Turns order snapshots ("filled 30 of 100 at 10.1 average") into the
/// individual fills since the last snapshot, so a partial fill is counted once.
#[derive(Default)]
pub struct FillTracker {
    seen: HashMap<String, (f64, f64, f64)>, // filled qty, notional, fees
}

impl FillTracker {
    pub fn update(&mut self, o: &Order) -> Option<Fill> {
        let seen = self.seen.entry(o.id.clone()).or_insert((0.0, 0.0, 0.0));
        let dq = o.filled_qty - seen.0;
        if dq <= 1e-12 {
            return None;
        }
        let notional = o.avg_price * o.filled_qty;
        // the first fill is the average itself (dividing would only add rounding noise)
        let price = if seen.0 <= 1e-12 { o.avg_price } else { (notional - seen.1) / dq };
        let fee = (o.fees - seen.2).max(0.0);
        *seen = (o.filled_qty, notional, o.fees);
        Some(Fill { order_id: o.id.clone(), symbol: o.symbol.clone(), side: o.side, qty: dq, price, fee })
    }

    pub fn forget_finished(&mut self, o: &Order) {
        if o.state.is_done() {
            self.seen.remove(&o.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, OrderState, Tif};

    fn order(filled: f64, avg: f64, fees: f64) -> Order {
        Order {
            id: "1".into(),
            client_id: "c".into(),
            symbol: "X".into(),
            side: Side::Buy,
            qty: 100.0,
            kind: Kind::Market,
            tif: Tif::Day,
            state: OrderState::PartiallyFilled,
            filled_qty: filled,
            avg_price: avg,
            fees,
            message: String::new(),
        }
    }

    #[test]
    fn partial_fills_are_counted_once() {
        let mut t = FillTracker::default();
        assert!(t.update(&order(0.0, 0.0, 0.0)).is_none());
        let a = t.update(&order(30.0, 10.0, 0.3)).unwrap();
        assert_eq!((a.qty, a.price, a.fee), (30.0, 10.0, 0.3));
        assert!(t.update(&order(30.0, 10.0, 0.3)).is_none());
        let b = t.update(&order(100.0, 11.0, 1.0)).unwrap();
        assert_eq!(b.qty, 70.0);
        assert!((b.price - (1100.0 - 300.0) / 70.0).abs() < 1e-9);
        assert!((b.fee - 0.7).abs() < 1e-9);
    }
}
