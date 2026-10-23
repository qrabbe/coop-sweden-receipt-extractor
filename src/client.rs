use crate::types::{now_ms, BankIdPoll, BankIdStart, ClientError, ReceiptListEntry, TokenSet};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use url::Url;

const SSO_HOST: &str = "https://sso.betala.coop.se";
const API_HOST: &str = "https://api.betala.coop.se";
const COOP_USER_AGENT: &str =
    "Coop/7.17; (11604; Production; Android 16; Android Build 36; Google; sdk_gphone64_x86_64)";
const SCANPAY_CLIENT_ID: &str = "scanpay";
const PER_PAGE: usize = 50;
const MAX_LIST_PAGES: usize = 100;

#[derive(Debug, Clone)]
pub struct CoopConfig {
    pub sso_base_url: String,
    pub api_base_url: String,
    pub poll_interval: Duration,
    pub login_timeout: Duration,
    pub request_timeout: Duration,
}

impl Default for CoopConfig {
    fn default() -> Self {
        Self {
            sso_base_url: SSO_HOST.to_string(),
            api_base_url: API_HOST.to_string(),
            poll_interval: Duration::from_secs(2),
            login_timeout: Duration::from_secs(300),
            request_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    config: CoopConfig,
}

impl Client {
    pub fn new(config: CoopConfig) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .user_agent(COOP_USER_AGENT)
            .timeout(config.request_timeout)
            .build()
            .map_err(http_err)?;
        Ok(Self { http, config })
    }
}

fn http_err(e: reqwest::Error) -> ClientError {
    ClientError::Http(e.to_string())
}

fn check_status(res: Response, expire_on: &[u16]) -> Result<Response, ClientError> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let code = status.as_u16();
    Err(if code == 429 {
        ClientError::RateLimited
    } else if expire_on.contains(&code) {
        ClientError::SessionExpired
    } else {
        ClientError::CoopUnavailable(code)
    })
}

async fn read_json(res: Response) -> Result<Value, ClientError> {
    let bytes = res.bytes().await.map_err(http_err)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| ClientError::CoopChanged("the response was not JSON".to_string()))
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn text_of(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

const BASE_BODY: [(&str, &str); 5] = [
    ("grant_type", "password"),
    ("scope", "openid offline_access"),
    ("username", ""),
    ("response_type", "id_token token"),
    ("client_id", SCANPAY_CLIENT_ID),
];
const POLL_EXTRA: [(&str, &str); 2] = [("required_personal_number", ""), ("user_visible_data", "")];

#[derive(Debug, Clone, Copy, Default)]
pub struct StartOptions {
    pub same_device: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginUpdate {
    Started(BankIdStart),
    Pending {
        qr_code: Option<String>,
        hint_code: Option<String>,
        auto_start_token: Option<String>,
    },
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    id_token: Option<String>,
    expires_in: Option<f64>,
    refresh_expires_in: Option<f64>,
    token_type: Option<String>,
    scope: Option<String>,
}

impl TokenResponse {
    fn into_token_set(self, now_ms: i64) -> TokenSet {
        let at = |secs: Option<f64>| {
            secs.filter(|s| *s > 0.0)
                .map(|s| now_ms + (s * 1000.0) as i64)
        };
        TokenSet {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            id_token: self.id_token,
            token_type: self.token_type,
            scope: self.scope,
            expires_at: at(self.expires_in).unwrap_or(0),
            refresh_expires_at: at(self.refresh_expires_in),
            obtained_at: now_ms,
        }
    }
}

fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

fn first_string(obj: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| obj.get(*k).and_then(Value::as_str))
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn opt_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}

impl Client {
    fn token_url(&self, order_ref: Option<&str>) -> Result<Url, ClientError> {
        let base = format!(
            "{}/auth/realms/coop/protocol/openid-connect/token",
            self.config.sso_base_url
        );
        let mut url =
            Url::parse(&base).map_err(|_| ClientError::Http("invalid SSO base URL".to_string()))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("origin", "scanpay");
            if let Some(order_ref) = order_ref {
                q.append_pair("orderRef", order_ref);
            }
        }
        Ok(url)
    }

    async fn post_form(&self, url: Url, body: String) -> Result<Response, ClientError> {
        self.http
            .post(url)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(http_err)
    }

    pub async fn start_bankid(&self, opts: StartOptions) -> Result<BankIdStart, ClientError> {
        let mut pairs = BASE_BODY.to_vec();
        pairs.push((
            "other_device",
            if opts.same_device { "false" } else { "true" },
        ));
        let res = self.post_form(self.token_url(None)?, form(&pairs)).await?;
        let json = read_json(check_status(res, &[])?).await?;

        let order_ref = first_string(&json, &["orderRef", "order_ref"]).ok_or_else(|| {
            ClientError::CoopChanged("the start response had no orderRef".to_string())
        })?;
        Ok(BankIdStart {
            order_ref,
            auto_start_token: first_string(&json, &["autostarttoken"]),
        })
    }

    pub async fn poll_bankid(&self, order_ref: &str) -> Result<BankIdPoll, ClientError> {
        let mut pairs = BASE_BODY.to_vec();
        pairs.extend(POLL_EXTRA);
        let res = self
            .post_form(self.token_url(Some(order_ref))?, form(&pairs))
            .await?;
        if !res.status().is_success() {
            return Ok(BankIdPoll::Failed {
                error: Some(format!("poll failed: {}", res.status().as_u16())),
                hint_code: None,
            });
        }
        let json = read_json(res).await?;

        if json.get("access_token").is_some_and(Value::is_string) {
            let raw: TokenResponse = serde_json::from_value(json)
                .map_err(|_| ClientError::CoopChanged("unexpected token response".to_string()))?;
            return Ok(BankIdPoll::Complete {
                tokens: raw.into_token_set(now_ms()),
            });
        }
        if let Some(error) = json.get("error").filter(|e| truthy(e)) {
            return Ok(BankIdPoll::Failed {
                error: Some(text_of(error)),
                hint_code: opt_string(&json, "hintCode"),
            });
        }
        Ok(BankIdPoll::Pending {
            qr_code: opt_string(&json, "qrCode"),
            hint_code: opt_string(&json, "hintCode"),
            auto_start_token: first_string(&json, &["autostarttoken"]),
        })
    }

    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenSet, ClientError> {
        let body = form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", SCANPAY_CLIENT_ID),
        ]);
        let res = self.post_form(self.token_url(None)?, body).await?;
        let json = read_json(check_status(res, &[400, 401, 403])?).await?;
        let raw: TokenResponse = serde_json::from_value(json)
            .map_err(|_| ClientError::CoopChanged("unexpected token response".to_string()))?;
        Ok(raw.into_token_set(now_ms()))
    }

    pub async fn login(
        &self,
        opts: StartOptions,
        mut on_update: impl FnMut(LoginUpdate),
    ) -> Result<TokenSet, ClientError> {
        let start = self.start_bankid(opts).await?;
        on_update(LoginUpdate::Started(start.clone()));

        let poll_loop = async {
            loop {
                match self.poll_bankid(&start.order_ref).await? {
                    BankIdPoll::Complete { tokens } => return Ok(tokens),
                    BankIdPoll::Failed { error, hint_code } => {
                        return Err(ClientError::LoginFailed { error, hint_code })
                    }
                    BankIdPoll::Pending {
                        qr_code,
                        hint_code,
                        auto_start_token,
                    } => on_update(LoginUpdate::Pending {
                        qr_code,
                        hint_code,
                        auto_start_token,
                    }),
                }
                tokio::time::sleep(self.config.poll_interval).await;
            }
        };
        tokio::time::timeout(self.config.login_timeout, poll_loop)
            .await
            .map_err(|_| ClientError::LoginTimedOut)?
    }
}

const TOKEN_REJECTED: [u16; 2] = [401, 403];

fn entry_from_row(row: Value) -> Result<ReceiptListEntry, ClientError> {
    let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    let id = text("receipt_id")
        .ok_or_else(|| ClientError::CoopChanged("a list row had no receipt_id".to_string()))?;
    Ok(ReceiptListEntry {
        id,
        purchased_at: text("purchased_at"),
        place: text("purchase_place"),
        amount: row.get("purchase_amount").and_then(Value::as_f64),
        raw: row,
    })
}

impl Client {
    fn api_url(&self, segments: &[&str]) -> Result<Url, ClientError> {
        let mut url = Url::parse(&self.config.api_base_url)
            .map_err(|_| ClientError::Http("invalid API base URL".to_string()))?;
        url.path_segments_mut()
            .map_err(|()| ClientError::Http("invalid API base URL".to_string()))?
            .pop_if_empty()
            .extend(["kvitto", "rest", "receipts", "v1"])
            .extend(segments);
        Ok(url)
    }

    pub async fn list_receipts(
        &self,
        access_token: &str,
    ) -> Result<Vec<ReceiptListEntry>, ClientError> {
        let mut entries: Vec<ReceiptListEntry> = Vec::new();

        for page in 1..=MAX_LIST_PAGES {
            let mut url = self.api_url(&[])?;
            url.query_pairs_mut()
                .append_pair("per_page", &PER_PAGE.to_string())
                .append_pair("page", &page.to_string());
            let res = self
                .http
                .get(url)
                .bearer_auth(access_token)
                .send()
                .await
                .map_err(http_err)?;
            let mut json = read_json(check_status(res, &TOKEN_REJECTED)?).await?;

            if let Some(error) = json.get("error").filter(|e| truthy(e)) {
                return Err(ClientError::CoopError(text_of(error)));
            }
            let rows = match json.get_mut("data").map(Value::take) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(rows)) => rows,
                Some(_) => {
                    return Err(ClientError::CoopChanged(
                        "`data` was not a list".to_string(),
                    ))
                }
            };
            let got = rows.len();
            for row in rows {
                entries.push(entry_from_row(row)?);
            }

            let total = json.get("total").and_then(Value::as_u64);
            let full_page = got >= PER_PAGE;
            let below_total = total.is_none_or(|t| (entries.len() as u64) < t);
            if !full_page || !below_total {
                return Ok(entries);
            }
        }
        Err(ClientError::CoopChanged(format!(
            "more than {MAX_LIST_PAGES} pages of receipts"
        )))
    }

    pub async fn fetch_receipt_pdf(
        &self,
        access_token: &str,
        id: &str,
    ) -> Result<Vec<u8>, ClientError> {
        let res = self
            .http
            .get(self.api_url(&[id, "formalReceipt"])?)
            .bearer_auth(access_token)
            .header(ACCEPT, "application/pdf;q=0.9,application/json;q=0.1")
            .send()
            .await
            .map_err(http_err)?;
        let res = check_status(res, &TOKEN_REJECTED)?;
        Ok(res.bytes().await.map_err(http_err)?.to_vec())
    }
}

pub fn bankid_app_link(auto_start_token: &str, mobile: bool) -> String {
    let token = percent_encode(auto_start_token);
    if mobile {
        format!("https://app.bankid.com/?autostarttoken={token}")
    } else {
        format!("bankid:///?autostarttoken={token}")
    }
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn hint_message(hint_code: Option<&str>) -> &'static str {
    match hint_code {
        Some("started" | "userSign") => "Confirm with your security code in the BankID app.",
        Some("userCancel" | "cancelled") => "The login was cancelled.",
        Some("expiredTransaction") => "The login took too long. Please try again.",
        _ => "Open the BankID app on your phone and scan the code.",
    }
}
