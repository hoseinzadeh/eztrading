//! The declarative adapter against the two fake brokerages, using the
//! shipped `brokers/*.json` descriptions with only the host swapped.

use ezt_broker::fake::{FakeServer, Flavor};
use ezt_broker::*;
use serde_json::Value;

fn spec(file: &str, base: &str) -> Value {
    let text = std::fs::read_to_string(format!("{}/../../brokers/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["base_url"] = base.into();
    // the real description points quotes at a different host; in the fake world there is one
    if let Some(q) = v["quote"]["path"].as_str() {
        if let Some(rest) = q.strip_prefix("https://data.alpaca.markets") {
            v["quote"]["path"] = rest.into();
        }
    }
    v
}

fn env(name: &str) -> Option<String> {
    Some(match name {
        "ALPACA_KEY_ID" => "key-123".into(),
        "ALPACA_SECRET_KEY" => "secret-456".into(),
        "TRADIER_TOKEN" => "tok-abcdef".into(),
        "TRADIER_ACCOUNT" => "ACC1".into(),
        _ => return None,
    })
}

fn req(side: Side, qty: f64, kind: Kind, id: &str) -> OrderRequest {
    OrderRequest { symbol: "ACME".into(), side, qty, kind, tif: Tif::Gtc, client_id: id.into() }
}

fn both() -> Vec<(FakeServer, RestBroker)> {
    let a = FakeServer::start(Flavor::Alpaca, "key-123", "", 10_000.0);
    let t = FakeServer::start(Flavor::Tradier, "tok-abcdef", "ACC1", 10_000.0);
    let ab = RestBroker::connect(&spec("alpaca-paper.json", &a.url()), &env).unwrap();
    let tb = RestBroker::connect(&spec("tradier-sandbox.json", &t.url()), &env).unwrap();
    for s in [&a, &t] {
        s.price("ACME", 50.0);
    }
    vec![(a, ab), (t, tb)]
}

#[test]
fn shipped_descriptions_parse_and_name_their_secrets() {
    for (file, names) in [("alpaca-paper.json", vec!["ALPACA_KEY_ID", "ALPACA_SECRET_KEY"]), ("tradier-sandbox.json", vec!["TRADIER_ACCOUNT", "TRADIER_TOKEN"])] {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/../../brokers/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
        let s = Spec::parse(&v).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(s.env_names(), names);
        assert!(s.limits.max_order_value.is_some());
    }
}

#[test]
fn account_positions_quotes_and_market_orders_on_both_dialects() {
    for (srv, mut b) in both() {
        assert_eq!(b.account().unwrap().cash, 10_000.0);
        assert!(b.positions().unwrap().is_empty());
        let q = b.quote("ACME").unwrap();
        assert!((q.last - 50.0).abs() < 0.02, "{q:?}");
        let o = b.place_order(&req(Side::Buy, 10.0, Kind::Market, "c-1")).unwrap();
        assert_eq!((o.state, o.filled_qty, o.avg_price), (OrderState::Filled, 10.0, 50.0), "{}", b.name());
        assert_eq!(o.client_id, "c-1");
        let ps = b.positions().unwrap();
        assert_eq!((ps.len(), ps[0].qty, ps[0].symbol.as_str()), (1, 10.0, "ACME"));
        assert_eq!(srv.positions()[0].qty, 10.0);
        assert!((b.account().unwrap().cash - 9_500.0).abs() < 1e-6);
        let s = b.place_order(&req(Side::Sell, 10.0, Kind::Market, "c-2")).unwrap();
        assert_eq!(s.state, OrderState::Filled);
        assert!(b.positions().unwrap().is_empty());
    }
}

#[test]
fn resting_orders_are_listed_cancelled_and_filled_by_the_market() {
    for (srv, mut b) in both() {
        let l = b.place_order(&req(Side::Buy, 5.0, Kind::Limit { limit: 45.0 }, "c-lim")).unwrap();
        assert_eq!(l.state, OrderState::Open, "{}", b.name());
        let s = b.place_order(&req(Side::Buy, 5.0, Kind::Limit { limit: 44.0 }, "c-lim2")).unwrap();
        assert_eq!(b.open_orders().unwrap().len(), 2);
        assert_eq!(b.cancel_order(&s.id).unwrap().state, OrderState::Canceled);
        let open = b.open_orders().unwrap(); // exactly one: an object (not array) in the Tradier dialect
        assert_eq!(open.len(), 1);
        srv.bar("ACME", 50.0, 50.0, 44.0, 46.0);
        let f = b.order(&l.id).unwrap();
        assert_eq!((f.state, f.filled_qty, f.avg_price), (OrderState::Filled, 5.0, 45.0));
        assert!(b.open_orders().unwrap().is_empty()); // "null" in the Tradier dialect
    }
}

#[test]
fn brokerage_refusals_and_outages_are_told_apart() {
    for (srv, mut b) in both() {
        let e = b.place_order(&req(Side::Buy, 1_000_000.0, Kind::Market, "huge")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Rejected, "{} {e}", b.name());
        let e = b.place_order(&req(Side::Sell, 1.0, Kind::Market, "none")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Rejected);
        srv.fail_next(5);
        let e = b.account().unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unavailable);
        srv.fail_next(1);
        assert!(b.account().is_ok(), "a single failed GET is retried");
    }
}

#[test]
fn wrong_credentials_are_an_auth_error_and_secrets_never_leak() {
    let a = FakeServer::start(Flavor::Alpaca, "other-key", "", 1.0);
    let mut b = RestBroker::connect(&spec("alpaca-paper.json", &a.url()), &env).unwrap();
    let e = b.account().unwrap_err();
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(!e.message.contains("key-123") && !e.message.contains("secret-456"));
    let missing = RestBroker::connect(&spec("alpaca-paper.json", &a.url()), &|_| None).err().unwrap();
    assert_eq!(missing.kind, ErrorKind::Auth);
    assert!(missing.message.contains("ALPACA_KEY_ID"));
}

#[test]
fn a_lost_reply_is_recovered_by_client_id_when_the_brokerage_supports_lookup() {
    let a = FakeServer::start(Flavor::Alpaca, "key-123", "", 10_000.0);
    a.price("ACME", 50.0);
    let mut b = RestBroker::connect(&spec("alpaca-paper.json", &a.url()), &env).unwrap();
    a.lose_next_reply();
    let o = b.place_order(&req(Side::Buy, 2.0, Kind::Market, "c-lost")).unwrap();
    assert_eq!((o.state, o.filled_qty), (OrderState::Filled, 2.0));
    assert_eq!(a.positions()[0].qty, 2.0, "placed once, not twice");

    // without a lookup endpoint the outcome is reported as unknown, never retried blindly
    let t = FakeServer::start(Flavor::Tradier, "tok-abcdef", "ACC1", 10_000.0);
    t.price("ACME", 50.0);
    let mut tb = RestBroker::connect(&spec("tradier-sandbox.json", &t.url()), &env).unwrap();
    t.lose_next_reply();
    let e = tb.place_order(&req(Side::Buy, 2.0, Kind::Market, "c-lost")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
    assert!(e.message.contains("may or may not"));
    assert_eq!(t.requests().iter().filter(|r| r.starts_with("POST")).count(), 1);
}

#[test]
fn a_guarded_rest_broker_enforces_limits_before_any_request_is_sent() {
    let a = FakeServer::start(Flavor::Alpaca, "key-123", "", 100_000.0);
    a.price("ACME", 50.0);
    let rb = RestBroker::connect(&spec("alpaca-paper.json", &a.url()), &env).unwrap();
    let limits = rb.spec().limits.clone();
    let mut g = Guarded::new(rb, limits, KillSwitch::default()).unwrap();
    let e = g.place_order(&req(Side::Buy, 100.0, Kind::Market, "big")).unwrap_err(); // 5000 > 2000
    assert_eq!(e.kind, ErrorKind::Blocked);
    assert!(a.requests().iter().all(|r| !r.starts_with("POST")));
    assert!(g.place_order(&req(Side::Buy, 10.0, Kind::Market, "ok")).is_ok());
}
