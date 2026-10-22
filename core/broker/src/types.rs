//! Plain data exchanged with a brokerage. Everything is serialisable so it can
//! be logged, replayed and shown in the UI.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Buy => "buy",
            Side::Sell => "sell",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    Market,
    Limit { limit: f64 },
    /// Becomes a market order once the price trades through `stop`.
    Stop { stop: f64 },
}

impl Kind {
    pub fn describe(&self) -> String {
        match self {
            Kind::Market => "market".into(),
            Kind::Limit { limit } => format!("limit {limit}"),
            Kind::Stop { stop } => format!("stop {stop}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tif {
    #[default]
    Day,
    Gtc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    pub kind: Kind,
    #[serde(default)]
    pub tif: Tif,
    /// Chosen by the caller; a broker must treat a repeated id as the same order.
    pub client_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderState {
    /// Accepted and working.
    Open,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Expired,
}

impl OrderState {
    pub fn is_done(self) -> bool {
        matches!(self, OrderState::Filled | OrderState::Canceled | OrderState::Rejected | OrderState::Expired)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            OrderState::Open => "open",
            OrderState::PartiallyFilled => "partially_filled",
            OrderState::Filled => "filled",
            OrderState::Canceled => "canceled",
            OrderState::Rejected => "rejected",
            OrderState::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub client_id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    pub kind: Kind,
    pub tif: Tif,
    pub state: OrderState,
    pub filled_qty: f64,
    /// Volume-weighted average fill price (0 until something fills).
    pub avg_price: f64,
    /// Total fees and commissions charged so far.
    pub fees: f64,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub cash: f64,
    pub equity: f64,
    pub buying_power: f64,
    #[serde(default)]
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub qty: f64,
    pub avg_cost: f64,
    pub market_value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub last: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// What a connection can touch. `Live` means real money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Simulated,
    Paper,
    Live,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrokerError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The brokerage said no (bad symbol, not enough funds, market closed...).
    Rejected,
    /// Could not reach it, timed out, or an unexpected reply.
    Unavailable,
    /// Our own safety layer refused.
    Blocked,
    /// This connection does not support the operation.
    Unsupported,
    /// Credentials missing or refused.
    Auth,
    /// A recorded run no longer matches what the strategy asks for.
    Diverged,
}

impl BrokerError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
    pub fn rejected(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Rejected, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unavailable, m)
    }
    pub fn blocked(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Blocked, m)
    }
    pub fn unsupported(m: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, m)
    }
}

impl fmt::Display for BrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BrokerError {}

pub type Result<T> = std::result::Result<T, BrokerError>;
