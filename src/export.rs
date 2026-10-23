//! ```text
//! coop-receipts-YYYY-MM-DD.zip
//!   manifest.json   tool version, time, counts, failures
//!   pdf/<date>_<receipt number>.pdf
//!   json/<date>_<receipt number>.json   parsed receipt and parse status
//! ```

use std::collections::HashSet;
use std::io::{Cursor, Write};

use coop_sweden_receipt_extractor::{
    extract_text, parse_text, ParseOptions, Receipt, ReceiptListEntry, VERSION,
};
use serde::Serialize;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub struct Fetched {
    pub entry: ReceiptListEntry,
    pub pdf: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseStatus {
    pub ok: bool,
    pub totals_match: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptJson {
    pub receipt: Option<Receipt>,
    pub parse: ParseStatus,
    /// Set only when parsing failed. The loyalty card number is masked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
}

fn redact(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.trim_start().starts_with("Medlemskort") {
                "Medlemskort ***"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn receipt_json(pdf: &[u8]) -> ReceiptJson {
    let failed = |warning: &str, raw_text: Option<String>| ReceiptJson {
        receipt: None,
        parse: ParseStatus {
            ok: false,
            totals_match: false,
            warnings: vec![warning.to_string()],
        },
        raw_text,
    };
    let Ok(text) = extract_text(pdf) else {
        return failed("The PDF text could not be read.", None);
    };
    let receipt = parse_text(&text, ParseOptions::default());
    if receipt.total.is_none() || receipt.items.is_empty() {
        return failed(
            "No total or items were found; the layout may be new.",
            Some(redact(&text)),
        );
    }
    let totals_match = receipt.totals_match();
    let warnings = if totals_match {
        Vec::new()
    } else {
        vec!["The items do not add up to the printed total.".to_string()]
    };
    ReceiptJson {
        receipt: Some(receipt),
        parse: ParseStatus {
            ok: true,
            totals_match,
            warnings,
        },
        raw_text: None,
    }
}

fn iso_time(now_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now_ms)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

pub fn zip_file_name(now_ms: i64) -> String {
    format!("coop-receipts-{}.zip", &iso_time(now_ms)[..10])
}

fn safe_file_name(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    out
}

/// `<date>_<receipt number>`, unique within the ZIP. Shared by the PDF and its JSON.
fn base_name(
    entry: &ReceiptListEntry,
    receipt: Option<&Receipt>,
    used: &mut HashSet<String>,
) -> String {
    let date: String = receipt
        .and_then(|r| r.purchased_at.as_deref())
        .or(entry.purchased_at.as_deref())
        .unwrap_or("unknown")
        .chars()
        .take(10)
        .collect();
    let number = receipt
        .and_then(|r| r.receipt_number.as_deref())
        .unwrap_or(&entry.id);
    let base = safe_file_name(&format!("{date}_{number}"));
    let mut name = base.clone();
    let mut n = 2;
    while used.contains(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    used.insert(name.clone());
    name
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    receipts: usize,
    pdfs: usize,
    parsed: usize,
    parse_failures: usize,
    totals_mismatch: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    tool_version: &'a str,
    exported_at: String,
    counts: Counts,
    failures: &'a [Failure],
}

/// The content of one `json/<name>.json`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptFile<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    place: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<f64>,
    #[serde(flatten)]
    json: &'a ReceiptJson,
}

pub fn build_zip(
    fetched: &[Fetched],
    failures: &[Failure],
    now_ms: i64,
) -> zip::result::ZipResult<Vec<u8>> {
    let parsed: Vec<Option<ReceiptJson>> = fetched
        .iter()
        .map(|f| f.pdf.as_deref().map(receipt_json))
        .collect();

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut used = HashSet::new();
    for (f, json) in fetched.iter().zip(&parsed) {
        let (Some(pdf), Some(json)) = (&f.pdf, json) else {
            continue;
        };
        let name = base_name(&f.entry, json.receipt.as_ref(), &mut used);
        zip.start_file(format!("pdf/{name}.pdf"), stored)?;
        zip.write_all(pdf)?;
        let file = ReceiptFile {
            id: &f.entry.id,
            place: f.entry.place.as_deref(),
            amount: f.entry.amount,
            json,
        };
        zip.start_file(format!("json/{name}.json"), deflated)?;
        zip.write_all(&serde_json::to_vec_pretty(&file).map_err(io_err)?)?;
    }

    let with_receipt = |j: &&ReceiptJson| j.receipt.is_some();
    let manifest = Manifest {
        tool_version: VERSION,
        exported_at: iso_time(now_ms),
        counts: Counts {
            receipts: fetched.len(),
            pdfs: fetched.iter().filter(|f| f.pdf.is_some()).count(),
            parsed: parsed.iter().flatten().filter(with_receipt).count(),
            parse_failures: parsed
                .iter()
                .flatten()
                .filter(|j| j.receipt.is_none())
                .count(),
            totals_mismatch: parsed
                .iter()
                .flatten()
                .filter(|j| j.receipt.is_some() && !j.parse.totals_match)
                .count(),
        },
        failures,
    };
    zip.start_file("manifest.json", deflated)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(io_err)?)?;
    Ok(zip.finish()?.into_inner())
}

fn io_err(e: serde_json::Error) -> std::io::Error {
    std::io::Error::other(e)
}
