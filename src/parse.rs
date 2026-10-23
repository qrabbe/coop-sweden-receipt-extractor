use crate::types::{LineItem, Receipt, Store, VatLine};
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseOptions {
    pub include_loyalty_card_id: bool,
    pub include_raw_text: bool,
}

pub fn parse_text(text: &str, opts: ParseOptions) -> Receipt {
    let meta = parse_metadata(text);
    Receipt {
        source: "coop".to_string(),
        store: meta.store,
        receipt_number: meta.receipt_number,
        purchased_at: meta.purchased_at,
        currency: "SEK".to_string(),
        total: meta.total,
        item_count: meta.item_count,
        discounts_total: meta.discounts_total,
        points_amount: meta.points_amount,
        vat: meta.vat,
        loyalty_card_id: if opts.include_loyalty_card_id {
            meta.loyalty_card_id
        } else {
            None
        },
        items: parse_items(text),
        cashier: meta.cashier,
        receipt_type: meta.receipt_type,
        raw_text: opts.include_raw_text.then(|| text.to_string()),
    }
}

fn clean_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

static POSTAL_PLACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]{5})\s+(.+)$").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^0[0-9\s]*-[0-9\s-]+$").unwrap());
static RECEIPT_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)kassakvitto").unwrap());
static DATE_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{4}-[0-9]{2}-[0-9]{2})[ T]([0-9]{2}:[0-9]{2})(?::([0-9]{2}))?").unwrap()
});
static VAT_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]+)%\s+([0-9\s,]+?)\s+([0-9\s,]+?)\s+([0-9\s,]+?)$").unwrap()
});

#[derive(Debug, Clone, Default, PartialEq)]
struct Metadata {
    store: Store,
    receipt_number: Option<String>,
    purchased_at: Option<String>,
    cashier: Option<String>,
    receipt_type: Option<String>,
    total: Option<f64>,
    item_count: Option<u32>,
    discounts_total: Option<f64>,
    points_amount: Option<f64>,
    vat: Vec<VatLine>,
    loyalty_card_id: Option<String>,
}

fn parse_metadata(text: &str) -> Metadata {
    let lines = clean_lines(text);
    let mut meta = Metadata {
        store: Store {
            name: lines.first().map_or("Unknown", |l| l).to_string(),
            ..Store::default()
        },
        ..Metadata::default()
    };

    for line in lines {
        if meta.store.postal_code.is_none() {
            if let Some(m) = POSTAL_PLACE.captures(line) {
                meta.store.postal_code = Some(m[1].to_string());
                meta.store.city = Some(m[2].to_string());
                continue;
            }
        }
        if meta.store.phone.is_none() && PHONE.is_match(line) {
            meta.store.phone = Some(line.to_string());
            continue;
        }
        if line.contains("ekonomisk förening") {
            meta.store.legal_entity = Some(line.to_string());
            continue;
        }
        if RECEIPT_TYPE.is_match(line) {
            meta.receipt_type = Some(line.to_string());
            continue;
        }

        if let Some(v) = value_after(line, "Kvitto ") {
            meta.receipt_number = Some(v.to_string());
        } else if let Some(v) = value_after(line, "Datum ") {
            meta.purchased_at = Some(to_iso(v));
        } else if let Some(v) = value_after(line, "Kassör ") {
            meta.cashier = Some(v.to_string());
        } else if let Some(v) = value_after(line, "Org.Nr ") {
            meta.store.org_nr = Some(v.to_string());
        } else if let Some(v) = value_after(line, "Total SEK ") {
            meta.total = parse_amount(v);
        } else if let Some(v) = value_after(line, "Antal artiklar ") {
            if let Some(n) = parse_int_prefix(v) {
                meta.item_count = Some(n);
            }
        } else if let Some(v) = value_after(line, "Erhållna rabatter ") {
            meta.discounts_total = parse_amount(v);
        } else if let Some(v) = value_after(line, "Poänggrundade belopp ") {
            meta.points_amount = parse_amount(v);
        } else if let Some(v) = value_after(line, "Medlemskort ") {
            meta.loyalty_card_id = Some(v.to_string());
        } else if let Some(m) = VAT_ROW.captures(line) {
            if let (Some(rate), Some(vat), Some(net), Some(gross)) = (
                parse_amount(&m[1]),
                parse_amount(&m[2]),
                parse_amount(&m[3]),
                parse_amount(&m[4]),
            ) {
                meta.vat.push(VatLine {
                    rate,
                    vat,
                    net,
                    gross,
                });
            }
        }
    }
    meta
}

fn value_after<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    line.strip_prefix(label)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn to_iso(raw: &str) -> String {
    match DATE_TIME.captures(raw) {
        Some(m) => format!(
            "{}T{}:{}",
            &m[1],
            &m[2],
            m.get(3).map_or("00", |s| s.as_str())
        ),
        None => raw.to_string(),
    }
}

static QUANTITY_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^x\s*([0-9]+(?:[.,][0-9]+)?)\s*(KG|STK|ST|ML|CL|DL|L|G)\s+-?[0-9]+[.,][0-9]{2}$",
    )
    .unwrap()
});
static UNIT_FRAGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+\.?[0-9]*\s+(KG|ST|L|ML|G)").unwrap());
static MULTIBUY_FRAGMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)FÖR\s+[0-9]+\s+KR").unwrap());
static NEGATIVE_PRICE_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-[0-9]+[.,][0-9]{2}$").unwrap());
static BARE_PRICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+,[0-9]{2}$").unwrap());
static PRICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?[0-9]+[.,][0-9]{2}").unwrap());
static PRICE_AT_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-?[0-9]+[.,][0-9]{2}$").unwrap());

#[derive(Debug, Clone, PartialEq)]
struct ExtractedLine {
    text: String,
    quantity: Option<f64>,
    unit: Option<String>,
}

fn parse_items(text: &str) -> Vec<LineItem> {
    extract_purchase_item_lines(text)
        .into_iter()
        .map(|l| {
            let price = parse_price(&l.text);
            LineItem {
                text: l.text,
                price,
                is_discount: price < 0.0,
                quantity: l.quantity,
                unit: l.unit,
                gtin: None,
            }
        })
        .collect()
}

fn extract_purchase_item_lines(text: &str) -> Vec<ExtractedLine> {
    let mut started = false;
    let mut out: Vec<ExtractedLine> = Vec::new();

    for line in clean_lines(text) {
        if !started {
            if line.contains("Org.Nr") {
                started = true;
            }
            continue;
        }
        if line.contains("Total SEK") {
            break;
        }

        if let Some(caps) = QUANTITY_LINE.captures(line) {
            if let Some(previous) = out.last_mut() {
                previous.quantity = parse_float_prefix(&caps[1].replacen(',', ".", 1));
                previous.unit = Some(normalize_unit(&caps[2]));
            }
            continue;
        }

        if UNIT_FRAGMENT.is_match(line)
            || (MULTIBUY_FRAGMENT.is_match(line) && !NEGATIVE_PRICE_END.is_match(line))
            || BARE_PRICE.is_match(line)
        {
            continue;
        }

        out.push(ExtractedLine {
            text: line.to_string(),
            quantity: None,
            unit: None,
        });
    }
    out
}

fn normalize_unit(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("stk") {
        "st".to_string()
    } else {
        raw.to_lowercase()
    }
}

fn parse_price(line: &str) -> f64 {
    PRICE_AT_END
        .find(line)
        .or_else(|| PRICE.find(line))
        .and_then(|m| parse_float_prefix(&m.as_str().replacen(',', ".", 1)))
        .unwrap_or(0.0)
}

static FLOAT_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?").unwrap()
});

fn parse_float_prefix(s: &str) -> Option<f64> {
    let m = FLOAT_PREFIX.find(s.trim_start())?;
    m.as_str().parse::<f64>().ok().filter(|n| n.is_finite())
}

fn parse_amount(raw: &str) -> Option<f64> {
    let cleaned: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    parse_float_prefix(&cleaned.replacen(',', ".", 1))
}

fn parse_int_prefix(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
