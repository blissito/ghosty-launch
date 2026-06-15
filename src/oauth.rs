//! OAuth 2.1 (PKCE, loopback) contra EasyBits + persistencia de credenciales.
//!
//! Flujo CLI estándar: registramos un cliente público con redirect a un puerto
//! loopback, abrimos el navegador a /oauth/authorize, capturamos el `code` en un
//! mini server local, y lo cambiamos por tokens en /oauth/token. El access token
//! (JWT 1h) entra con scope ADMIN; el refresh token (90d) permite reconectar sin
//! reabrir el navegador. Se guardan en ~/.config/ghosty-launch/credentials.json.

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub fn base_url() -> String {
    std::env::var("EASYBITS_BASE_URL")
        .unwrap_or_else(|_| "https://www.easybits.cloud".to_string())
        .trim_end_matches('/')
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creds {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: u64, // unix secs en que expira el access
    #[serde(default)]
    pub client_id: Option<String>,
}

impl Creds {
    /// Vencido (con 60s de colchón).
    pub fn is_expired(&self) -> bool {
        now() + 60 >= self.expires_at
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn creds_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    Some(base.join("ghosty-launch").join("credentials.json"))
}

pub fn load_creds() -> Option<Creds> {
    let path = creds_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_creds(creds: &Creds) {
    let Some(path) = creds_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(creds) {
        if std::fs::write(&path, json).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

pub fn clear_creds() {
    if let Some(path) = creds_path() {
        let _ = std::fs::remove_file(path);
    }
}

fn rand_b64(n: usize) -> Result<String> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|e| anyhow!("rng: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    let _ = std::process::Command::new(cmd).arg(url).spawn();
}

fn query_get(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(
                    urlencoding::decode(v)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| v.to_string()),
                );
            }
        }
    }
    None
}

fn creds_from_token_json(v: &Value, client_id: Option<String>) -> Result<Creds> {
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("respuesta sin access_token"))?
        .to_string();
    let refresh = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .map(String::from);
    let expires_in = v.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(3600);
    Ok(Creds {
        access_token: access,
        refresh_token: refresh,
        expires_at: now() + expires_in,
        client_id,
    })
}

/// Flujo completo PKCE loopback. Abre el navegador y bloquea hasta el callback.
pub async fn run_oauth() -> Result<Creds> {
    let base = base_url();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/cb");
    let http = reqwest::Client::new();

    // 1. Registro dinámico de cliente público.
    let reg: Value = http
        .post(format!("{base}/oauth/register"))
        .json(&serde_json::json!({
            "client_name": "Ghosty Launch",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let client_id = reg
        .get("client_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("registro sin client_id"))?
        .to_string();

    // 2. PKCE S256 + state.
    let verifier = rand_b64(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = rand_b64(16)?;

    // 3. Navegador → /oauth/authorize.
    let auth_url = format!(
        "{base}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&scope=mcp&state={}",
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        challenge,
        urlencoding::encode(&state),
    );
    open_url(&auth_url);

    // 4. Esperar el callback.
    let (code, got_state) = wait_for_code(&listener).await?;
    if got_state != state {
        return Err(anyhow!("state inválido (posible CSRF)"));
    }

    // 5. Cambiar code por tokens.
    let tok: Value = http
        .post(format!("{base}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("client_id", &client_id),
            ("code_verifier", &verifier),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let creds = creds_from_token_json(&tok, Some(client_id))?;
    save_creds(&creds);
    Ok(creds)
}

/// Cambia el refresh token (rotándolo) por un par nuevo. Reconexión sin navegador.
pub async fn refresh(creds: &Creds) -> Result<Creds> {
    let base = base_url();
    let rt = creds
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow!("sin refresh token"))?;
    let cid = creds
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow!("sin client_id"))?;
    let http = reqwest::Client::new();
    let tok: Value = http
        .post(format!("{base}/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt),
            ("client_id", cid),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut new = creds_from_token_json(&tok, creds.client_id.clone())?;
    if new.refresh_token.is_none() {
        new.refresh_token = creds.refresh_token.clone();
    }
    save_creds(&new);
    Ok(new)
}

async fn wait_for_code(listener: &TcpListener) -> Result<(String, String)> {
    let (mut stream, _) = listener.accept().await?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let code = query_get(path, "code");
    let state = query_get(path, "state").unwrap_or_default();

    let body = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Ghosty Launch</title></head>\
        <body style=\"font-family:ui-monospace,Menlo,monospace;display:grid;place-items:center;height:100vh;margin:0;background:#0b0b0f;color:#a29be8\">\
        <div style=\"text-align:center\">\
        <pre style=\"color:#a29be8;font-size:22px;line-height:1.05;margin:0\"> \u{2584}\u{2588}\u{2588}\u{2588}\u{2588}\u{2584} \n\u{2590} \u{25d1}  \u{25d1} \u{258c}\n\u{2590}\u{2588}\u{2580}\u{2588}\u{2588}\u{2580}\u{2588}\u{258c}</pre>\
        <h2 style=\"margin:18px 0 4px;font-weight:600\">Conectado</h2>\
        <p style=\"color:#787882;margin:0\">Vuelve a la terminal — ya puedes cerrar esta pesta\u{f1}a.</p>\
        </div></body></html>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;

    match code {
        Some(c) => Ok((c, state)),
        None => Err(anyhow!(
            "el callback no trajo 'code' (¿autorización cancelada?)"
        )),
    }
}
