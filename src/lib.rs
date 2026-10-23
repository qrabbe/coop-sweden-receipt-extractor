#[cfg(feature = "client")]
pub mod client;
pub mod parse;
pub mod types;

#[cfg(feature = "client")]
pub use client::{Client, CoopConfig, LoginUpdate, StartOptions};
pub use parse::{parse_text, ParseOptions};
#[cfg(feature = "client")]
pub use types::ClientError;
pub use types::{
    now_ms, BankIdPoll, BankIdStart, LineItem, ParseError, Receipt, ReceiptListEntry, Store,
    TokenSet, VatLine,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn parse_pdf(bytes: &[u8], opts: ParseOptions) -> Result<Receipt, ParseError> {
    Ok(parse_text(&extract_text(bytes)?, opts))
}

pub fn extract_text(bytes: &[u8]) -> Result<String, ParseError> {
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .map_err(|_| ParseError::Pdf("extractor panicked".to_string()))?
        .map_err(|e| ParseError::Pdf(e.to_string()))?;
    let lines: Vec<String> = text
        .split('\n')
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    Ok(lines.join("\n"))
}
