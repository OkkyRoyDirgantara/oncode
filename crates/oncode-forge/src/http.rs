//! Perkakas HTTP bersama: klien, pemetaan galat, paginasi, dan parse waktu.

use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, ACCEPT, AUTHORIZATION, RETRY_AFTER};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{ForgeError, ForgeErrorCode, Result};

pub(crate) const USER_AGENT: &str = "OnCode";
/// Ukuran halaman untuk semua daftar yang ditampilkan ke pengguna.
pub(crate) const PER_PAGE: u32 = 30;
/// Ukuran halaman untuk daftar yang dipindai habis di sisi klien.
pub(crate) const SCAN_PER_PAGE: u32 = 100;

/// String rahasia yang tidak pernah tercetak lewat `Debug`.
#[derive(Clone)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(value: &str) -> Self {
        Self(value.to_string())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"***\"")
    }
}

/// Membuat klien HTTP baru. Klien tidak dibagi secara global: pool koneksinya terikat pada
/// runtime async tempat koneksi dibuat, sehingga klien bersama lintas runtime (seperti pada
/// uji, tiap uji punya runtime sendiri) bisa kehilangan koneksi di tengah permintaan.
pub(crate) fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| ForgeError::network(format!("Could not set up the HTTP client: {}", err.without_url())))
}

/// Detik sejak epoch Unix menurut jam lokal.
pub(crate) fn now_epoch() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Jawaban HTTP yang sudah dibaca seluruhnya.
pub(crate) struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Mengirim permintaan dan membaca seluruh badan jawaban. Galat transport menjadi
/// `Network`; status HTTP belum diperiksa di sini.
pub(crate) async fn send(request: RequestBuilder, server: &str) -> Result<Reply> {
    let response = request.send().await.map_err(|err| transport_error(err, server))?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(|err| transport_error(err, server))?.to_vec();
    Ok(Reply { status, headers, body })
}

fn transport_error(err: reqwest::Error, server: &str) -> ForgeError {
    // URL dibuang dari pesan; rinciannya cukup jenis kegagalannya.
    let err = err.without_url();
    let message = if err.is_timeout() {
        format!("Timed out while contacting {server}. Check your internet connection and try again.")
    } else if err.is_connect() {
        format!("Could not connect to {server}. Check your internet connection or the server address.")
    } else {
        format!("Request to {server} failed: {err}")
    };
    ForgeError::network(message)
}

/// Gaya API, menentukan header bawaan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    GitHub,
    GitLab,
}

/// Klien REST untuk satu instance dan satu token.
#[derive(Debug, Clone)]
pub(crate) struct Api {
    flavor: Flavor,
    /// Basis REST tanpa garis miring di akhir.
    base: String,
    /// Nama server untuk pesan galat, misalnya `github.com`.
    server: String,
    token: Secret,
    /// Klien milik penyedia ini; dibuat saat pertama dipakai dan dipakai ulang untuk semua permintaannya.
    client: OnceLock<reqwest::Client>,
}

impl Api {
    pub(crate) fn new(flavor: Flavor, base: &str, server: &str, token: &str) -> Self {
        Self {
            flavor,
            base: base.trim_end_matches('/').to_string(),
            server: server.to_string(),
            token: Secret::new(token),
            client: OnceLock::new(),
        }
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let url = format!("{}{}", self.base, path);
        let client = match self.client.get() {
            Some(client) => client.clone(),
            None => {
                let created = client()?;
                self.client.get_or_init(|| created).clone()
            }
        };
        let request = client
            .request(method, url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token.expose()));
        Ok(match self.flavor {
            Flavor::GitHub => request
                .header(ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28"),
            Flavor::GitLab => request.header(ACCEPT, "application/json"),
        })
    }

    /// `GET` yang mengembalikan JSON beserta header (untuk paginasi).
    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<(T, HeaderMap)> {
        let request = self.request(Method::GET, path)?.query(query);
        let reply = send(request, &self.server).await?;
        let value = self.decode(&reply)?;
        Ok((value, reply.headers))
    }

    /// `POST` dengan badan JSON.
    pub(crate) async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let request = self.request(Method::POST, path)?.json(body);
        let reply = send(request, &self.server).await?;
        self.decode(&reply)
    }

    fn decode<T: DeserializeOwned>(&self, reply: &Reply) -> Result<T> {
        if !reply.status.is_success() {
            let mut error = status_error(reply, &self.server);
            // Jaga-jaga bila server menggemakan kredensial di pesan galatnya.
            let token = self.token.expose();
            if !token.is_empty() && error.message.contains(token) {
                error.message = error.message.replace(token, "***");
            }
            return Err(error);
        }
        serde_json::from_slice(&reply.body).map_err(|err| {
            ForgeError::invalid_response(format!("The response from {} could not be understood: {err}", self.server))
        })
    }
}

/// Memetakan status HTTP selain 2xx ke [`ForgeError`].
pub(crate) fn status_error(reply: &Reply, server: &str) -> ForgeError {
    let status = reply.status;
    let detail = server_message(&reply.body);
    let with_detail = |base: String| match &detail {
        Some(detail) => format!("{base} ({detail})"),
        None => base,
    };

    let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN && is_rate_limited(&reply.headers, detail.as_deref()));
    if rate_limited {
        let wait = retry_after_seconds(&reply.headers);
        let message = match wait {
            Some(seconds) => format!("API rate limit for {server} exceeded. Try again in {seconds} seconds."),
            None => format!("API rate limit for {server} exceeded. Try again in a little while."),
        };
        return ForgeError::new(ForgeErrorCode::RateLimited, message);
    }

    match status.as_u16() {
        401 => ForgeError::new(
            ForgeErrorCode::Unauthorized,
            with_detail(format!("The login session for {server} is no longer valid. Please log in again.")),
        ),
        403 => ForgeError::new(
            ForgeErrorCode::Forbidden,
            with_detail(format!("This account does not have permission for this action on {server}")),
        ),
        404 => ForgeError::new(
            ForgeErrorCode::NotFound,
            with_detail(format!("Not found on {server}, or this account does not have access")),
        ),
        400 | 409 | 422 => ForgeError::invalid(with_detail(format!("Request rejected by {server}"))),
        code if status.is_server_error() => {
            ForgeError::network(with_detail(format!("{server} is having problems (HTTP {code}). Try again later.")))
        }
        code => ForgeError::invalid_response(with_detail(format!("Unexpected response from {server} (HTTP {code})"))),
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

fn is_rate_limited(headers: &HeaderMap, detail: Option<&str>) -> bool {
    if headers.contains_key(RETRY_AFTER) {
        return true;
    }
    // GitHub: `x-ratelimit-remaining`; GitLab: `ratelimit-remaining`.
    let exhausted = ["x-ratelimit-remaining", "ratelimit-remaining"]
        .iter()
        .any(|name| header_str(headers, name) == Some("0"));
    exhausted || detail.is_some_and(|d| d.to_ascii_lowercase().contains("rate limit"))
}

/// Berapa detik lagi boleh mencoba, dari `retry-after` atau waktu reset batas laju.
fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    if let Some(seconds) = header_str(headers, "retry-after").and_then(|v| v.parse::<u64>().ok()) {
        return Some(seconds);
    }
    let reset = ["x-ratelimit-reset", "ratelimit-reset"]
        .iter()
        .find_map(|name| header_str(headers, name).and_then(|v| v.parse::<f64>().ok()))?;
    let wait = reset - now_epoch();
    (wait > 0.0).then(|| wait.ceil() as u64)
}

/// Mengambil pesan galat dari badan JSON, dengan toleransi pada bentuk yang dipakai
/// GitHub (`message`, `errors[].message`), GitLab (`message` berupa string, larik, atau
/// objek; `error`), dan OAuth (`error_description`).
pub(crate) fn server_message(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    let mut parts: Vec<String> = Vec::new();
    for key in ["message", "error_description", "error"] {
        if let Some(found) = object.get(key) {
            flatten_message(found, &mut parts);
            if !parts.is_empty() {
                break;
            }
        }
    }
    if let Some(errors) = object.get("errors").and_then(|e| e.as_array()) {
        for item in errors {
            match item {
                serde_json::Value::String(text) => push_unique(&mut parts, text),
                serde_json::Value::Object(map) => {
                    if let Some(text) = map.get("message").and_then(|m| m.as_str()) {
                        push_unique(&mut parts, text);
                    }
                }
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut message = parts.join("; ");
    if message.chars().count() > 300 {
        message = message.chars().take(300).collect::<String>() + "…";
    }
    Some(message)
}

fn push_unique(parts: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() && !parts.iter().any(|p| p == text) {
        parts.push(text.to_string());
    }
}

fn flatten_message(value: &serde_json::Value, parts: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => push_unique(parts, text),
        serde_json::Value::Array(items) => items.iter().for_each(|item| flatten_message(item, parts)),
        serde_json::Value::Object(map) => {
            for (field, item) in map {
                let mut inner = Vec::new();
                flatten_message(item, &mut inner);
                if !inner.is_empty() {
                    push_unique(parts, &format!("{field}: {}", inner.join(", ")));
                }
            }
        }
        _ => {}
    }
}

/// Halaman berikutnya dari header `Link` (GitHub): nilai parameter `page` pada tautan
/// `rel="next"`.
pub(crate) fn next_page_from_link(headers: &HeaderMap) -> Option<u32> {
    let link = header_str(headers, "link")?;
    for part in link.split(',') {
        let mut pieces = part.split(';');
        let target = pieces.next()?.trim();
        let is_next = pieces.any(|p| {
            let p = p.trim();
            p == "rel=\"next\"" || p == "rel=next"
        });
        if !is_next {
            continue;
        }
        let url = target.strip_prefix('<')?.strip_suffix('>')?;
        let query = url.split_once('?')?.1;
        let query = query.split('#').next().unwrap_or_default();
        return query
            .split('&')
            .find_map(|pair| pair.strip_prefix("page="))
            .and_then(|v| v.parse::<u32>().ok());
    }
    None
}

/// Halaman berikutnya dari header `x-next-page` (GitLab); kosong berarti sudah habis.
pub(crate) fn next_page_from_gitlab(headers: &HeaderMap) -> Option<u32> {
    match header_str(headers, "x-next-page") {
        Some(value) if !value.is_empty() => value.parse::<u32>().ok(),
        Some(_) => None,
        None => next_page_from_link(headers),
    }
}

/// Meng-encode satu segmen path. Semua selain karakter *unreserved* (RFC 3986) diubah
/// ke `%XX`, termasuk `/`, sehingga `grup/proyek` menjadi `grup%2Fproyek`.
pub(crate) fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Mem-parse waktu ISO 8601 / RFC 3339 (`2024-05-01T12:34:56.789Z`,
/// `2024-05-01T19:34:56+07:00`) menjadi detik sejak epoch Unix.
pub(crate) fn parse_timestamp(text: &str) -> Option<f64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19 || !text.is_ascii() {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    let separators_ok = bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b't' | b' ')
        && bytes[13] == b':'
        && bytes[16] == b':';
    if !separators_ok || !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60
    {
        return None;
    }

    let mut rest = &text[19..];
    let mut fraction = 0.0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        fraction = format!("0.{}", &after_dot[..digits]).parse::<f64>().ok()?;
        rest = &after_dot[digits..];
    }

    let offset_seconds = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = rest[1..].chars().filter(|c| *c != ':').collect();
            if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let hours = digits[0..2].parse::<i64>().ok()?;
            let minutes = digits[2..4].parse::<i64>().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };

    // Hari sejak epoch untuk kalender Gregorian proleptik (algoritme "days from civil").
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    Some(seconds as f64 + fraction)
}

/// Waktu opsional dari API menjadi [`Timestamp`](crate::model::Timestamp); 0 bila tidak
/// ada atau tidak terbaca.
pub(crate) fn timestamp_or_zero(text: Option<&str>) -> f64 {
    text.and_then(parse_timestamp).unwrap_or(0.0)
}

/// Menjadikan string kosong sebagai `None`.
pub(crate) fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_timestamp("2011-01-26T19:01:12Z"), Some(1_296_068_472.0));
        assert_eq!(parse_timestamp("2024-02-29T23:59:59Z"), Some(1_709_251_199.0));
        assert_eq!(parse_timestamp("2024-05-01T12:34:56.500Z"), Some(1_714_566_896.5));
        // Zona waktu: 19:34:56+07:00 sama dengan 12:34:56Z.
        assert_eq!(parse_timestamp("2024-05-01T19:34:56+07:00"), Some(1_714_566_896.0));
        assert_eq!(parse_timestamp("2024-05-01T07:04:56.000-05:30"), Some(1_714_566_896.0));
        assert_eq!(parse_timestamp("2024-05-01T19:34:56+0700"), Some(1_714_566_896.0));
        assert_eq!(parse_timestamp("2024-05-01 12:34:56"), Some(1_714_566_896.0));
        assert_eq!(parse_timestamp("1969-12-31T23:59:59Z"), Some(-1.0));
        assert_eq!(parse_timestamp(""), None);
        assert_eq!(parse_timestamp("kemarin sore"), None);
        assert_eq!(parse_timestamp("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_timestamp("2024-05-01T12:34:56+7"), None);
        assert_eq!(timestamp_or_zero(None), 0.0);
    }

    #[test]
    fn next_page_from_link_header() {
        let both = headers(&[(
            "link",
            "<https://api.github.com/user/repos?sort=updated&per_page=30&page=3>; rel=\"next\", \
             <https://api.github.com/user/repos?sort=updated&per_page=30&page=9>; rel=\"last\"",
        )]);
        assert_eq!(next_page_from_link(&both), Some(3));
        let last_only = headers(&[(
            "link",
            "<https://api.github.com/user/repos?page=1>; rel=\"prev\", <https://api.github.com/user/repos?page=1>; rel=\"first\"",
        )]);
        assert_eq!(next_page_from_link(&last_only), None);
        // `per_page` tidak boleh terbaca sebagai `page`.
        let per_page_first = headers(&[("link", "<https://x/y?per_page=30&page=2>; rel=\"next\"")]);
        assert_eq!(next_page_from_link(&per_page_first), Some(2));
        assert_eq!(next_page_from_link(&HeaderMap::new()), None);
    }

    #[test]
    fn next_page_from_gitlab_header() {
        assert_eq!(next_page_from_gitlab(&headers(&[("x-next-page", "4")])), Some(4));
        assert_eq!(next_page_from_gitlab(&headers(&[("x-next-page", "")])), None);
        assert_eq!(next_page_from_gitlab(&HeaderMap::new()), None);
        assert_eq!(
            next_page_from_gitlab(&headers(&[("link", "<https://gitlab.com/api/v4/projects?page=2&per_page=30>; rel=\"next\"")])),
            Some(2)
        );
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(encode_segment("grup/subgrup/proyek"), "grup%2Fsubgrup%2Fproyek");
        assert_eq!(encode_segment("nama.repo-1_x~"), "nama.repo-1_x~");
        assert_eq!(encode_segment("a b#c"), "a%20b%23c");
    }

    #[test]
    fn extracts_server_messages() {
        assert_eq!(server_message(br#"{"message":"Bad credentials"}"#).as_deref(), Some("Bad credentials"));
        assert_eq!(
            server_message(br#"{"message":"Validation Failed","errors":[{"message":"A pull request already exists"}]}"#)
                .as_deref(),
            Some("Validation Failed; A pull request already exists")
        );
        assert_eq!(
            server_message(br#"{"message":["Another open merge request already exists"]}"#).as_deref(),
            Some("Another open merge request already exists")
        );
        assert_eq!(
            server_message(br#"{"message":{"title":["can't be blank"]}}"#).as_deref(),
            Some("title: can't be blank")
        );
        assert_eq!(
            server_message(br#"{"error":"invalid_token","error_description":"Token is expired."}"#).as_deref(),
            Some("Token is expired.")
        );
        assert_eq!(server_message(br#"{"error":"insufficient_scope"}"#).as_deref(), Some("insufficient_scope"));
        assert_eq!(server_message(b"<html>502</html>"), None);
    }

    #[test]
    fn secret_debug_is_redacted() {
        let api = Api::new(Flavor::GitHub, "https://api.github.com/", "github.com", "gho_sangat_rahasia");
        let debug = format!("{api:?}");
        assert!(!debug.contains("gho_sangat_rahasia"), "{debug}");
        assert!(debug.contains("***"));
    }
}
