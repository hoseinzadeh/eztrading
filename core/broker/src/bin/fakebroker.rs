//! `ezt-fakebroker`: a stand-in brokerage on localhost, for trying connections
//! and strategies without an account. It imitates the *shape* of two public
//! APIs; it is not those services.
//!
//!     ezt-fakebroker --flavor alpaca --key demo --price ACME=100
//!     curl -X POST localhost:18900/__control/price -d '{"symbol":"ACME","price":92}'
//!     curl -X POST localhost:18900/__control/bar   -d '{"symbol":"ACME","open":92,"high":95,"low":90,"close":94}'
//!     curl -X POST localhost:18900/__control/fail  -d '{"count":3}'   # next 3 requests get 503

use ezt_broker::fake::{FakeServer, Flavor};

fn main() {
    let mut flavor = Flavor::Alpaca;
    let (mut port, mut key, mut account, mut cash) = (18900u16, "demo".to_owned(), "ACC1".to_owned(), 100_000.0f64);
    let mut prices: Vec<(String, f64)> = vec![];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = |what: &str| args.next().unwrap_or_else(|| usage(&format!("{what} needs a value")));
        match a.as_str() {
            "--flavor" => {
                flavor = match next("--flavor").as_str() {
                    "alpaca" => Flavor::Alpaca,
                    "tradier" => Flavor::Tradier,
                    other => usage(&format!("unknown flavor `{other}`")),
                }
            }
            "--port" => port = next("--port").parse().unwrap_or_else(|_| usage("bad port")),
            "--key" => key = next("--key"),
            "--account" => account = next("--account"),
            "--cash" => cash = next("--cash").parse().unwrap_or_else(|_| usage("bad cash")),
            "--price" => {
                let v = next("--price");
                let (s, p) = v.split_once('=').unwrap_or_else(|| usage("--price SYMBOL=PRICE"));
                prices.push((s.to_owned(), p.parse().unwrap_or_else(|_| usage("bad price"))));
            }
            "-h" | "--help" => usage(""),
            other => usage(&format!("unknown argument `{other}`")),
        }
    }
    let server = FakeServer::start_on(&format!("127.0.0.1:{port}"), flavor, &key, &account, cash);
    for (s, p) in prices {
        server.price(&s, p);
    }
    println!("fake brokerage ({}) at {}  key `{key}`  account `{account}`  cash {cash}", if flavor == Flavor::Alpaca { "alpaca-like" } else { "tradier-like" }, server.url());
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

fn usage(problem: &str) -> ! {
    if !problem.is_empty() {
        eprintln!("error: {problem}");
    }
    eprintln!("usage: ezt-fakebroker [--flavor alpaca|tradier] [--port 18900] [--key demo] [--account ACC1] [--cash 100000] [--price SYMBOL=PRICE]...");
    std::process::exit(if problem.is_empty() { 0 } else { 2 });
}
