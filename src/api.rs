//! REST API that lets external apps push their own dashboard pages into the
//! rotation. A page is declarative: a title plus a list of widgets (headings,
//! text rows, bars, graphs, tables) that `monit` renders with its framebuffer
//! primitives — apps describe *what* to show, not *where* to draw it.
//!
//! The server is a tiny blocking HTTP/1.1 handler over `TcpListener` in a
//! background thread; JSON is parsed with serde. Pages live in a shared,
//! TTL-expiring store; apps re-POST to keep a page alive and refresh its data.
//!
//! Endpoints (base `/api/v1`):
//!   POST   /api/v1/pages        upsert a page (JSON body, see `PushedPage`)
//!   GET    /api/v1/pages        list pages (id, title, widget count, age, ttl)
//!   GET    /api/v1/pages/{id}   echo a stored page
//!   DELETE /api/v1/pages/{id}   remove a page
//!   GET    /healthz             liveness (never requires auth)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_BODY: usize = 256 * 1024; // reject larger request bodies
const MAX_WIDGETS: usize = 64;
const MAX_SERIES: usize = 1024; // cap graph points kept per widget
const DEFAULT_TTL: u64 = 60; // seconds a page survives without a refresh

// ---------------------------------------------------------------------------
// Declarative page model
// ---------------------------------------------------------------------------

#[derive(Deserialize, Serialize, Clone)]
pub struct PushedPage {
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// Seconds the page lives without a refreshing POST. 0 = never expire.
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Widget {
    /// A section heading.
    Heading {
        text: String,
        #[serde(default)]
        color: Option<String>,
    },
    /// A label + value row (label optional → free text).
    Text {
        #[serde(default)]
        label: Option<String>,
        value: String,
        #[serde(default)]
        color: Option<String>,
    },
    /// A labelled progress/utilization bar. `frac` is 0..1; `value` is an
    /// optional right-aligned caption (e.g. "12.3 GB").
    Bar {
        label: String,
        frac: f64,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    /// A history/area graph. `series` is raw values; if `max` is given they are
    /// normalized by it, otherwise auto-scaled to the series maximum.
    Graph {
        #[serde(default)]
        label: Option<String>,
        series: Vec<f64>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        max: Option<f64>,
    },
    /// A simple text table. `columns` is an optional header row.
    Table {
        #[serde(default)]
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct StoredPage {
    pub page: PushedPage,
    pub updated: Instant,
    pub expires_at: Option<Instant>,
}

/// Ordered by id so the rotation is stable across refreshes.
pub type Store = Arc<Mutex<BTreeMap<String, StoredPage>>>;

pub fn new_store() -> Store {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// Drop expired pages and return the surviving ids in rotation order.
pub fn active_ids(store: &Store, now: Instant) -> Vec<String> {
    let mut s = store.lock().unwrap();
    s.retain(|_, v| v.expires_at.map(|e| e > now).unwrap_or(true));
    s.keys().cloned().collect()
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

pub struct ApiConfig {
    pub bind: String,         // e.g. "0.0.0.0:9090"; empty/"off" disables
    pub token: Option<String>, // optional bearer token
}

/// Spawn the API server in a background thread. Returns immediately; on bind
/// failure it logs to stderr and the dashboard keeps running without the API.
pub fn serve(cfg: ApiConfig, store: Store) {
    if cfg.bind.is_empty() || cfg.bind.eq_ignore_ascii_case("off") {
        return;
    }
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(&cfg.bind) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("monit: API disabled, cannot bind {}: {e}", cfg.bind);
                return;
            }
        };
        eprintln!("monit: API listening on {}", cfg.bind);
        let token = Arc::new(cfg.token);
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let store = store.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &store, token.as_ref().as_ref());
            });
        }
    });
}

struct Request {
    method: String,
    path: String,
    auth: Option<String>,
    body: Vec<u8>,
}

fn handle(mut stream: TcpStream, store: &Store, token: Option<&String>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let req = match read_request(&mut stream)? {
        Some(r) => r,
        None => return respond(&mut stream, 400, "text/plain", b"bad request"),
    };

    // Liveness never needs auth.
    if req.method == "GET" && req.path == "/healthz" {
        return respond(&mut stream, 200, "text/plain", b"ok");
    }

    // Optional bearer-token gate on everything else.
    if let Some(tok) = token {
        let ok = req
            .auth
            .as_deref()
            .and_then(|a| a.strip_prefix("Bearer "))
            .map(|t| t == tok)
            .unwrap_or(false);
        if !ok {
            return respond(&mut stream, 401, "application/json", br#"{"error":"unauthorized"}"#);
        }
    }

    let path = req.path.clone();
    let rest = path.strip_prefix("/api/v1/pages");
    match (req.method.as_str(), rest) {
        ("POST", Some("")) | ("POST", Some("/")) => post_page(&mut stream, store, &req.body),
        ("GET", Some("")) | ("GET", Some("/")) => list_pages(&mut stream, store),
        ("GET", Some(p)) => get_page(&mut stream, store, p.trim_start_matches('/')),
        ("DELETE", Some(p)) => delete_page(&mut stream, store, p.trim_start_matches('/')),
        _ => respond(&mut stream, 404, "application/json", br#"{"error":"not found"}"#),
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    // Read until headers are complete (CRLFCRLF), then the declared body.
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];
    let header_end;
    loop {
        match stream.read(&mut tmp)? {
            0 => return Ok(None),
            n => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
        if buf.len() > 64 * 1024 {
            return Ok(None); // header section too large
        }
    }

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let start = lines.next().unwrap_or("");
    let mut it = start.split_whitespace();
    let method = it.next().unwrap_or("").to_string();
    let path = it.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Ok(None);
    }

    let mut content_len = 0usize;
    let mut auth = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            match k.as_str() {
                "content-length" => content_len = v.parse().unwrap_or(0),
                "authorization" => auth = Some(v.to_string()),
                _ => {}
            }
        }
    }
    if content_len > MAX_BODY {
        return Ok(None);
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_len {
        match stream.read(&mut tmp)? {
            0 => break,
            n => body.extend_from_slice(&tmp[..n]),
        }
    }
    body.truncate(content_len);
    Ok(Some(Request { method, path, auth, body }))
}

fn post_page(stream: &mut TcpStream, store: &Store, body: &[u8]) -> std::io::Result<()> {
    let mut page: PushedPage = match serde_json::from_slice(body) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!(r#"{{"error":"invalid json: {}"}}"#, escape(&e.to_string()));
            return respond(stream, 400, "application/json", msg.as_bytes());
        }
    };
    if page.id.trim().is_empty() {
        return respond(stream, 400, "application/json", br#"{"error":"id is required"}"#);
    }
    // Clamp resource use from untrusted input.
    page.widgets.truncate(MAX_WIDGETS);
    for w in &mut page.widgets {
        if let Widget::Graph { series, .. } = w {
            if series.len() > MAX_SERIES {
                let start = series.len() - MAX_SERIES;
                *series = series.split_off(start);
            }
        }
    }
    let ttl = page.ttl_secs.unwrap_or(DEFAULT_TTL);
    let now = Instant::now();
    let expires_at = if ttl == 0 { None } else { Some(now + Duration::from_secs(ttl)) };
    let id = page.id.clone();
    store
        .lock()
        .unwrap()
        .insert(id.clone(), StoredPage { page, updated: now, expires_at });
    let msg = format!(r#"{{"ok":true,"id":"{}"}}"#, escape(&id));
    respond(stream, 200, "application/json", msg.as_bytes())
}

fn list_pages(stream: &mut TcpStream, store: &Store) -> std::io::Result<()> {
    let now = Instant::now();
    let s = store.lock().unwrap();
    let items: Vec<_> = s
        .values()
        .map(|v| {
            serde_json::json!({
                "id": v.page.id,
                "title": v.page.title,
                "widgets": v.page.widgets.len(),
                "age_secs": now.duration_since(v.updated).as_secs(),
                "expires_in": v.expires_at.map(|e| e.saturating_duration_since(now).as_secs()),
            })
        })
        .collect();
    let body = serde_json::to_vec(&serde_json::json!({ "pages": items })).unwrap_or_default();
    respond(stream, 200, "application/json", &body)
}

fn get_page(stream: &mut TcpStream, store: &Store, id: &str) -> std::io::Result<()> {
    let s = store.lock().unwrap();
    match s.get(id) {
        Some(v) => {
            let body = serde_json::to_vec(&v.page).unwrap_or_default();
            respond(stream, 200, "application/json", &body)
        }
        None => respond(stream, 404, "application/json", br#"{"error":"not found"}"#),
    }
}

fn delete_page(stream: &mut TcpStream, store: &Store, id: &str) -> std::io::Result<()> {
    let removed = store.lock().unwrap().remove(id).is_some();
    let code = if removed { 200 } else { 404 };
    let body = if removed { br#"{"ok":true}"#.as_ref() } else { br#"{"error":"not found"}"#.as_ref() };
    respond(stream, code, "application/json", body)
}

fn respond(stream: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Minimal JSON string escaping for the small messages we build by hand.
fn escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            '\n' | '\r' | '\t' => vec![' '],
            c => vec![c],
        })
        .collect()
}
