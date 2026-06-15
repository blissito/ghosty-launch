//! Modo headless de debug: corre el pipeline imprimiendo TODO crudo a stdout,
//! sin TUI. La forma correcta de debuggear la integración con la API.
//!
//! Uso:  EASYBITS_API_KEY=eb_sk_… cargo run -- --debug

use crate::app::{App, APP_PORT, TEMPLATE};
use crate::easybits::Client;
use anyhow::{anyhow, Result};

pub async fn run() -> Result<()> {
    let key = std::env::var("EASYBITS_API_KEY")
        .map_err(|_| anyhow!("falta EASYBITS_API_KEY en el entorno"))?;
    let client = Client::new(key)?;
    let base = std::env::var("EASYBITS_BASE_URL")
        .unwrap_or_else(|_| "https://www.easybits.cloud".to_string());
    println!("== base: {base}");
    println!("== template: {TEMPLATE}  persistent: true  port: {APP_PORT}");
    println!("== ref repo: {}\n", App::ref_repo());

    // 1) validar
    let (st, body) = client.get_raw("/me").await?;
    println!("[GET /me] {st}\n{}\n", trunc(&body, 400));
    if st != 200 {
        return Err(anyhow!("validación falló"));
    }

    // 2) crear sandbox (crudo, para ver el shape exacto)
    let (st, body) = client
        .post_raw(
            "/sandboxes",
            serde_json::json!({ "template": TEMPLATE, "persistent": true }),
        )
        .await?;
    println!("[POST /sandboxes] {st}\n{}\n", trunc(&body, 800));
    if st >= 400 {
        return Err(anyhow!("create falló"));
    }
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let id = v
        .get("sandboxId")
        .or_else(|| v.get("id"))
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("sin sandboxId en la respuesta"))?
        .to_string();
    println!("== sandboxId: {id}\n");

    // 3) poll de estado, imprimiendo el body completo cada vuelta
    let mut running = false;
    for i in 0..40 {
        let (st, body) = client.get_raw(&format!("/sandboxes/{id}")).await?;
        let status = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_default();
        println!(
            "[poll {i}] http {st} status={status:?}  {}",
            trunc(&body, 240)
        );
        if status == "running" {
            running = true;
            break;
        }
        if status == "error" || status == "lost" {
            return Err(anyhow!("la VM entró en estado terminal: {status}"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    if !running {
        println!("\n== NO llegó a running. Revisa arriba el último status.");
        return Ok(());
    }
    println!("\n== running ✓\n");

    // 4) deploy
    let cmd = format!(
        "set -e; rm -rf /app; git clone --depth 1 {} /app; cd /app; if [ -f package-lock.json ]; then npm ci --omit=dev; else npm install --omit=dev; fi; (PORT={APP_PORT} nohup npm start > /tmp/app.log 2>&1 &); sleep 3; echo started",
        App::ref_repo()
    );
    let (st, body) = client
        .post_raw(
            &format!("/sandboxes/{id}/exec"),
            serde_json::json!({ "command": cmd, "timeoutSeconds": 300 }),
        )
        .await?;
    println!("[exec deploy] {st}\n{}\n", trunc(&body, 1200));

    // 5) expose
    let (st, body) = client
        .post_raw(
            &format!("/sandboxes/{id}/expose"),
            serde_json::json!({ "port": APP_PORT }),
        )
        .await?;
    println!("[expose] {st}\n{}\n", trunc(&body, 400));

    println!("== fin. (la VM {id} sigue viva — destrúyela con DELETE si no la quieres)");
    Ok(())
}

fn trunc(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}
