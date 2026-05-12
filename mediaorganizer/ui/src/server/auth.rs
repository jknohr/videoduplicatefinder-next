//! Web UI authentication — port of VDF.Web/Services/AuthService.cs.
//!
//! On first launch a random 10-character password is generated, saved to the
//! platform config directory, and printed to stdout so Docker users can see it
//! with `docker logs`. The password can be overridden via VDF_WEB_PASSWORD or
//! disabled entirely with VDF_WEB_AUTH=false.
//!
//! Tokens: random 32-byte hex strings issued on successful login, stored in
//! memory and checked via the `vdf_auth` HTTP-only cookie on every request.
//! Tokens survive as long as the server process is running (30-day cookie max-age).

use std::collections::HashSet;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

const COOKIE_NAME: &str = "vdf_auth";
const COOKIE_MAX_AGE_DAYS: u64 = 30;
const PASSWORD_CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789";

struct AuthState {
    password: String,
    enabled: bool,
    tokens: HashSet<String>,
}

static AUTH: OnceLock<Mutex<AuthState>> = OnceLock::new();

/// Initialise the authentication subsystem. Must be called once on startup
/// before any request is processed.
pub fn init_auth() {
    AUTH.get_or_init(|| {
        let auth_env = std::env::var("VDF_WEB_AUTH").unwrap_or_default();
        let enabled = !auth_env.eq_ignore_ascii_case("false");

        let password = if enabled {
            let pw = load_or_generate_password();
            print_password_banner(&pw);
            pw
        } else {
            info!("Web UI authentication is DISABLED (VDF_WEB_AUTH=false).");
            String::new()
        };

        Mutex::new(AuthState { password, enabled, tokens: HashSet::new() })
    });
}

/// Returns `true` if the provided token (from the `vdf_auth` cookie) is valid.
pub fn validate_token(token: &str) -> bool {
    match AUTH.get() {
        None => false,
        Some(m) => {
            let state = m.lock().unwrap();
            if !state.enabled { return true; }
            state.tokens.contains(token)
        }
    }
}

/// Returns `true` if authentication is enabled.
pub fn is_auth_enabled() -> bool {
    match AUTH.get() {
        None => true,
        Some(m) => m.lock().unwrap().enabled,
    }
}

/// Validate a password. On success, issue and return a token.
/// Returns `None` if the password is wrong.
pub fn login(password: &str) -> Option<String> {
    let mutex = AUTH.get()?;
    let mut state = mutex.lock().ok()?;
    if !state.enabled {
        return Some("disabled".to_string());
    }
    if state.password != password {
        return None;
    }
    let token = generate_random_hex();
    state.tokens.insert(token.clone());
    Some(token)
}

// ── Login HTML endpoint ───────────────────────────────────────────────────────

/// Axum GET handler — serves a simple login HTML form.
pub async fn login_page() -> axum::response::Html<&'static str> {
    axum::response::Html(LOGIN_HTML)
}

/// Axum POST handler — validates the submitted password and sets the auth cookie.
pub async fn login_submit(
    form: axum::extract::Form<LoginForm>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let token = login(&form.password);
    match token {
        Some(tok) => {
            let max_age = COOKIE_MAX_AGE_DAYS * 24 * 3600;
            let cookie = format!(
                "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
                COOKIE_NAME, tok, max_age
            );
            axum::response::Response::builder()
                .status(302)
                .header("Location", "/")
                .header("Set-Cookie", cookie)
                .body(axum::body::Body::empty())
                .unwrap()
                .into_response()
        }
        None => {
            axum::response::Html(LOGIN_HTML_BAD).into_response()
        }
    }
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    password: String,
}

/// Axum middleware — checks the vdf_auth cookie on every request.
///
/// If auth is disabled, passes through. If auth is enabled and the cookie
/// is missing or invalid, redirects to /login for HTML requests and returns
/// 401 for /api/* requests.
pub async fn auth_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // Auth disabled → allow everything
    if !is_auth_enabled() {
        return next.run(req).await;
    }

    // Allow /login endpoints through without auth check
    let path = req.uri().path().to_string();
    if path == "/login" || path == "/auth/login" {
        return next.run(req).await;
    }

    // Extract vdf_auth cookie from request headers
    let token = req.headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie_header| {
            cookie_header.split(';')
                .find(|part| part.trim().starts_with(COOKIE_NAME))
                .and_then(|part| part.splitn(2, '=').nth(1))
                .map(|v| v.trim().to_string())
        });

    let authenticated = token.as_deref().map(validate_token).unwrap_or(false);
    if authenticated {
        return next.run(req).await;
    }

    // Not authenticated
    if path.starts_with("/api/") {
        // API calls → 401 JSON
        (axum::http::StatusCode::UNAUTHORIZED,
         axum::Json(serde_json::json!({ "error": "Unauthorized" }))).into_response()
    } else {
        // Browser navigation → redirect to login page
        axum::response::Response::builder()
            .status(302)
            .header("Location", "/login")
            .body(axum::body::Body::empty())
            .unwrap()
            .into_response()
    }
}

// ── Password management ───────────────────────────────────────────────────────

fn load_or_generate_password() -> String {
    // Priority: env var > saved file > generate new
    if let Ok(env_pw) = std::env::var("VDF_WEB_PASSWORD") {
        if !env_pw.trim().is_empty() {
            return env_pw.trim().to_string();
        }
    }

    let creds_path = credentials_path();

    if let Some(ref p) = creds_path {
        if p.exists() {
            if let Ok(contents) = std::fs::read_to_string(p) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
                    if let Some(pw) = json.get("password").and_then(|v| v.as_str()) {
                        if !pw.is_empty() {
                            return pw.to_string();
                        }
                    }
                }
            }
        }
    }

    let pw = generate_password();
    if let Some(p) = creds_path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::json!({ "password": pw });
        let _ = std::fs::write(p, serde_json::to_string_pretty(&json).unwrap());
    }
    pw
}

fn generate_password() -> String {
    let mut bytes = [0u8; 10];
    fill_random_bytes(&mut bytes);
    bytes.iter()
        .map(|&b| PASSWORD_CHARS[b as usize % PASSWORD_CHARS.len()] as char)
        .collect()
}

fn generate_random_hex() -> String {
    let mut bytes = [0u8; 32];
    fill_random_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fill_random_bytes(buf: &mut [u8]) {
    // Try /dev/urandom first (Linux/macOS)
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(buf).is_ok() { return; }
    }
    // Fallback: mix of time + process ID (weaker, but better than zeros)
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(42) as u64;
    let pid = std::process::id() as u64;
    let mut x = seed ^ (pid << 32);
    for b in buf.iter_mut() {
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 56) as u8;
    }
}

fn credentials_path() -> Option<std::path::PathBuf> {
    dirs::config_local_dir().map(|d| d.join("vdf").join("web_credentials.json"))
}

fn print_password_banner(password: &str) {
    let source = if std::env::var("VDF_WEB_PASSWORD").is_ok() {
        " (from VDF_WEB_PASSWORD env var)"
    } else {
        ""
    };
    info!("============================================");
    info!("  Web UI password:  {}{}", password, source);
    info!("============================================");
    info!("  Set VDF_WEB_AUTH=false to disable authentication.");
    info!("  Set VDF_WEB_PASSWORD=<password> to use a custom password.");

    println!();
    println!("============================================");
    println!("  Web UI password:  {}{}", password, source);
    println!("============================================");
    println!();
}

// ── Login page HTML ───────────────────────────────────────────────────────────

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>MediaOrganizer — Login</title>
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: system-ui, sans-serif; background: #0f1117; color: #e0e0e0;
         display: flex; align-items: center; justify-content: center; min-height: 100vh; }
  .card { background: #1a1d24; border: 1px solid #2a2d36; border-radius: 12px;
          padding: 2.5rem 3rem; width: 100%; max-width: 360px; }
  h1 { font-size: 1.3rem; font-weight: 600; margin-bottom: 1.5rem;
       text-align: center; color: #fff; }
  label { display: block; font-size: 0.85rem; color: #9ca3af; margin-bottom: 0.4rem; }
  input[type=password] { width: 100%; padding: 0.6rem 0.8rem; border-radius: 6px;
    border: 1px solid #374151; background: #111827; color: #f9fafb;
    font-size: 1rem; outline: none; }
  input[type=password]:focus { border-color: #6366f1; }
  button { width: 100%; margin-top: 1.2rem; padding: 0.7rem; border-radius: 6px;
    background: #6366f1; color: #fff; font-size: 1rem; font-weight: 600;
    border: none; cursor: pointer; }
  button:hover { background: #4f46e5; }
</style>
</head>
<body>
<div class="card">
  <h1>MediaOrganizer</h1>
  <form method="POST" action="/auth/login">
    <label for="password">Password</label>
    <input type="password" id="password" name="password" autofocus autocomplete="current-password" required>
    <button type="submit">Sign in</button>
  </form>
</div>
</body>
</html>"#;

const LOGIN_HTML_BAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>MediaOrganizer — Login</title>
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: system-ui, sans-serif; background: #0f1117; color: #e0e0e0;
         display: flex; align-items: center; justify-content: center; min-height: 100vh; }
  .card { background: #1a1d24; border: 1px solid #2a2d36; border-radius: 12px;
          padding: 2.5rem 3rem; width: 100%; max-width: 360px; }
  h1 { font-size: 1.3rem; font-weight: 600; margin-bottom: 1.5rem;
       text-align: center; color: #fff; }
  label { display: block; font-size: 0.85rem; color: #9ca3af; margin-bottom: 0.4rem; }
  input[type=password] { width: 100%; padding: 0.6rem 0.8rem; border-radius: 6px;
    border: 1px solid #374151; background: #111827; color: #f9fafb;
    font-size: 1rem; outline: none; }
  input[type=password]:focus { border-color: #6366f1; }
  button { width: 100%; margin-top: 1.2rem; padding: 0.7rem; border-radius: 6px;
    background: #6366f1; color: #fff; font-size: 1rem; font-weight: 600;
    border: none; cursor: pointer; }
  button:hover { background: #4f46e5; }
  .error { background: #7f1d1d; color: #fca5a5; border-radius: 6px;
           padding: 0.6rem 0.8rem; font-size: 0.875rem; margin-bottom: 1rem; }
</style>
</head>
<body>
<div class="card">
  <h1>MediaOrganizer</h1>
  <div class="error">Incorrect password. Please try again.</div>
  <form method="POST" action="/auth/login">
    <label for="password">Password</label>
    <input type="password" id="password" name="password" autofocus autocomplete="current-password" required>
    <button type="submit">Sign in</button>
  </form>
</div>
</body>
</html>"#;
