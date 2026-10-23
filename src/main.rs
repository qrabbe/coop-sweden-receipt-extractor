mod export;

use coop_sweden_receipt_extractor::client::{bankid_app_link, hint_message};
use coop_sweden_receipt_extractor::{
    now_ms, Client, ClientError, CoopConfig, LoginUpdate, ReceiptListEntry, StartOptions, TokenSet,
};
use export::{Failure, Fetched};
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

const USAGE: &str = "usage: coop-receipts [export] [-o|--output PATH] [--same-device]
       coop-receipts parse FILE.pdf...
       coop-receipts -h | --help | -V | --version";
const CONCURRENCY: usize = 4;

enum Command {
    Export {
        output: Option<PathBuf>,
        same_device: bool,
    },
    Parse(Vec<PathBuf>),
    Help,
    Version,
}

fn parse_args(args: &[String]) -> Option<Command> {
    let rest = match args.split_first() {
        Some((f, r)) if f == "parse" => {
            let files: Vec<PathBuf> = r.iter().map(PathBuf::from).collect();
            return (!files.is_empty()).then_some(Command::Parse(files));
        }
        Some((f, r)) if f == "export" => r,
        _ => args,
    };
    let (mut output, mut same_device) = (None, false);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Some(Command::Help),
            "-V" | "--version" => return Some(Command::Version),
            "--same-device" => same_device = true,
            "-o" | "--output" => output = Some(PathBuf::from(it.next()?)),
            _ => return None,
        }
    }
    Some(Command::Export {
        output,
        same_device,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match parse_args(&args) {
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        Some(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(Command::Version) => {
            println!("coop-receipts {}", coop_sweden_receipt_extractor::VERSION);
            return ExitCode::SUCCESS;
        }
        Some(Command::Parse(files)) => parse_files(&files),
        Some(Command::Export {
            output,
            same_device,
        }) => run_export(output, same_device).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type BoxError = Box<dyn std::error::Error>;

fn parse_files(files: &[PathBuf]) -> Result<(), BoxError> {
    let mut parsed = Vec::new();
    for path in files {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        parsed.push(serde_json::to_value(export::receipt_json(&bytes))?);
    }
    let out = if parsed.len() == 1 {
        parsed.remove(0)
    } else {
        parsed.into()
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

async fn run_export(output: Option<PathBuf>, same_device: bool) -> Result<(), BoxError> {
    let path = output.unwrap_or_else(|| PathBuf::from(export::zip_file_name(now_ms())));
    if path.exists() {
        return Err(format!("{} already exists, not overwriting it", path.display()).into());
    }
    let client = Client::new(CoopConfig::default())?;
    let tokens = login(&client, same_device).await?;
    eprintln!("Logged in. Listing receipts...");
    let entries = client.list_receipts(&tokens.access_token).await?;
    let (fetched, failures) = download_all(&client, tokens, entries).await?;

    let zip = export::build_zip(&fetched, &failures, now_ms())?;
    std::fs::write(&path, zip).map_err(|e| format!("{}: {e}", path.display()))?;
    let pdfs = fetched.iter().filter(|f| f.pdf.is_some()).count();
    eprintln!(
        "Wrote {}\n{} receipts, {pdfs} PDFs, {} failures",
        path.display(),
        fetched.len(),
        failures.len()
    );
    Ok(())
}

async fn login(client: &Client, same_device: bool) -> Result<TokenSet, ClientError> {
    let mut last_qr = String::new();
    client
        .login(StartOptions { same_device }, |update| match update {
            LoginUpdate::Started(start) => {
                if let (true, Some(token)) = (same_device, &start.auto_start_token) {
                    eprintln!(
                        "Open this link on this device: {}",
                        bankid_app_link(token, false)
                    );
                }
            }
            LoginUpdate::Pending {
                qr_code, hint_code, ..
            } => {
                if let Some(qr) = qr_code.filter(|q| *q != last_qr) {
                    if let Ok(code) = QrCode::new(qr.as_bytes()) {
                        let image = code.render::<Dense1x2>().quiet_zone(true).build();
                        eprintln!(
                            "\x1b[2J\x1b[H{image}\n{}",
                            hint_message(hint_code.as_deref())
                        );
                    }
                    last_qr = qr;
                }
            }
        })
        .await
}

type Outcome = Result<Vec<u8>, Stop>;
enum Stop {
    Failed(String),
    Abort(String),
}

async fn download_all(
    client: &Client,
    tokens: TokenSet,
    entries: Vec<ReceiptListEntry>,
) -> Result<(Vec<Fetched>, Vec<Failure>), BoxError> {
    let tokens = Arc::new(Mutex::new(tokens));
    let total = entries.len();
    let mut results: Vec<Option<Vec<u8>>> = vec![None; total];
    let mut failures = Vec::new();
    let (mut done, mut next) = (0, 0);
    let mut tasks: JoinSet<(usize, Outcome)> = JoinSet::new();

    while done < total {
        while next < total && tasks.len() < CONCURRENCY {
            let (client, tokens, id) = (client.clone(), tokens.clone(), entries[next].id.clone());
            let index = next;
            tasks.spawn(async move { (index, fetch_one(&client, &tokens, &id).await) });
            next += 1;
        }
        let (index, outcome) = tasks
            .join_next()
            .await
            .ok_or("download tasks ended early")??;
        done += 1;
        match outcome {
            Ok(pdf) => results[index] = Some(pdf),
            Err(Stop::Failed(message)) => failures.push(Failure {
                id: entries[index].id.clone(),
                message,
            }),
            Err(Stop::Abort(message)) => {
                eprintln!();
                return Err(message.into());
            }
        }
        eprint!(
            "\r\x1b[KDownloading {done}/{total} ({} failed)",
            failures.len()
        );
    }
    if total > 0 {
        eprintln!();
    }
    let fetched = entries
        .into_iter()
        .zip(results)
        .map(|(entry, pdf)| Fetched { entry, pdf })
        .collect();
    Ok((fetched, failures))
}

async fn fetch_one(client: &Client, tokens: &Mutex<TokenSet>, id: &str) -> Outcome {
    let mut retried = false;
    let mut refreshed = false;
    loop {
        let used = tokens.lock().await.access_token.clone();
        let err = match client.fetch_receipt_pdf(&used, id).await {
            Ok(pdf) => return Ok(pdf),
            Err(e) => e,
        };
        if matches!(err, ClientError::SessionExpired) {
            if refreshed {
                return Err(Stop::Abort(
                    "Coop rejected the refreshed session".to_string(),
                ));
            }
            refreshed = true;
            let mut current = tokens.lock().await;
            if current.access_token == used {
                match client.refresh(&current.refresh_token).await {
                    Ok(new) => *current = new,
                    Err(e) => {
                        return Err(Stop::Abort(format!("could not refresh the session: {e}")))
                    }
                }
            }
            continue;
        }
        if retried {
            return Err(Stop::Failed(err.to_string()));
        }
        retried = true;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
