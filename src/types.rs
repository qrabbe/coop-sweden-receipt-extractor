use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_nr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_entity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VatLine {
    pub rate: f64,
    pub vat: f64,
    pub net: f64,
    pub gross: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineItem {
    pub text: String,
    pub price: f64,
    pub is_discount: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gtin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub source: String,
    pub store: Store,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchased_at: Option<String>,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discounts_total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points_amount: Option<f64>,
    pub vat: Vec<VatLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loyalty_card_id: Option<String>,
    pub items: Vec<LineItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cashier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
}

impl Receipt {
    pub fn items_total(&self) -> f64 {
        self.items.iter().map(|i| i.price).sum()
    }

    pub fn totals_match(&self) -> bool {
        self.total
            .is_some_and(|total| ((self.items_total() - total) * 100.0).round().abs() <= 1.0)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// `0` if unknown.
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_expires_at: Option<i64>,
    #[serde(default)]
    pub obtained_at: i64,
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("expires_at", &self.expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .field("obtained_at", &self.obtained_at)
            .finish_non_exhaustive()
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BankIdStart {
    pub order_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BankIdPoll {
    Pending {
        qr_code: Option<String>,
        hint_code: Option<String>,
        auto_start_token: Option<String>,
    },
    Complete {
        tokens: TokenSet,
    },
    Failed {
        error: Option<String>,
        hint_code: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptListEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchased_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("could not read the PDF: {0}")]
    Pdf(String),
}

#[cfg(feature = "client")]
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("session expired")]
    SessionExpired,
    #[error("Coop is unavailable (HTTP {0})")]
    CoopUnavailable(u16),
    #[error("rate limited by Coop")]
    RateLimited,
    #[error("Coop changed: {0}")]
    CoopChanged(String),
    #[error("Coop returned an error: {0}")]
    CoopError(String),
    #[error("BankID login failed: {}", .error.as_deref().unwrap_or("unknown"))]
    LoginFailed {
        error: Option<String>,
        hint_code: Option<String>,
    },
    #[error("BankID login timed out")]
    LoginTimedOut,
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("network error: {0}")]
    Http(String),
}
