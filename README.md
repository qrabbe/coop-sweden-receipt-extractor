# Coop receipt extractor

A command-line tool that logs in to Coop Sweden with BankID and downloads all your
receipts in one ZIP: the original PDFs, plus the parsed data as JSON. The
parser is also a Rust library.

**Unofficial.** Coop did not make this tool. It works for
Coop in Sweden only, and only for your own receipts.

## Use it

Download a binary from the GitHub releases, or install with Rust 1.88 or newer:

```sh
cargo install --git https://github.com/qrabbe/coop-sweden-receipt-extractor --features cli
```

```sh
coop-receipts                     # scan the BankID QR code and get a zip
coop-receipts -o receipts.zip     # choose the file name
coop-receipts --same-device       # open BankID on this computer instead of a QR code
coop-receipts parse receipt.pdf   # parse PDFs you already have
```

The ZIP has the PDFs in `pdf/`, the parsed data for each one in `json/` (same file
name), and `manifest.json` with counts and any download failures.

## Use the library

```toml
coop-sweden-receipt-extractor = "0.1"                                      # parser only
coop-sweden-receipt-extractor = { version = "0.1", features = ["client"] } # plus the Coop client
```

```rust
use coop_sweden_receipt_extractor::{parse_pdf, ParseOptions};

let receipt = parse_pdf(&pdf_bytes, ParseOptions::default())?;
println!("{:?} {}", receipt.total, receipt.totals_match()); // do the items add up?
```

`parse_text` takes text you extracted yourself. Fields it cannot find are `None`, and
`totals_match()` tells you if the layout was understood. The `client` feature adds
`Client` with BankID login, the receipt list and PDF download. The client is experimental.

## How it is built

| File | Job |
|---|---|
| `src/lib.rs` | Public API and PDF text extraction |
| `src/parse.rs` | Receipt text to typed `Receipt`, with a total check |
| `src/client.rs` | BankID login, receipt list and PDF download over `reqwest` and `rustls` |
| `src/export.rs` | The ZIP: one PDF and one JSON file per receipt |
| `src/main.rs` | The CLI |
| `src/types.rs` | Data types and errors |

The repository has no automated tests, because real receipts are private data. To check
the parser, put your own PDFs in `private/` (gitignored) and run
`coop-receipts parse private/*.pdf`. An unknown layout shows as `"totalsMatch": false`
or as `"receipt": null` with the raw text, so wrong data never passes silently.