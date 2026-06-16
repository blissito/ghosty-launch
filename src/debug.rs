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

/// Prueba e2e REAL del agente de errores, sin TUI. Deploya un repo (que esperamos
/// falle), y cuando falla suelta al agente sobre la VM viva. Reusa el código real:
/// `spawn_launch` (deploy) + `spawn_fix_agent` (agente) + el `Client` de EasyBits.
///
/// Uso:  cargo run -- --agent-e2e https://github.com/owner/repo
/// (la inferencia va por EasyBits con tu sesión guardada — sin key extra).
pub async fn agent_e2e(repo: String) -> Result<()> {
    use crate::app::Msg;
    use std::time::Duration;
    use tokio::sync::mpsc;

    // 1) Client de EasyBits: EASYBITS_API_KEY, o las credenciales OAuth guardadas.
    let client = build_client().await?;
    client.validate().await.map_err(|e| anyhow!("validación EasyBits falló: {e}"))?;
    println!("== EasyBits OK (la misma llave hostea sandboxes y da inferencia)");

    // 2) Deploy real del repo. Drenamos los Msg igual que el TUI.
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    println!("== deployando {repo} …\n");
    crate::app::spawn_launch(
        client.clone(),
        tx.clone(),
        repo,
        "agenda-e2e".into(),
        "#7c3aed".into(),
        String::new(),
        Vec::new(),
    );

    let mut sandbox_id: Option<String> = None;
    let mut failed_err: Option<String> = None;
    loop {
        match tokio::time::timeout(Duration::from_secs(900), rx.recv()).await {
            Ok(Some(Msg::SandboxCreated { id })) => {
                println!("   VM creada: {id}");
                sandbox_id = Some(id);
            }
            Ok(Some(Msg::Step { idx, status, detail })) => {
                println!("   [paso {idx}] {} {detail}", step_label(status));
            }
            Ok(Some(Msg::Live { url })) => {
                println!("\n🟢 LIVE: {url}\n== el deploy NO falló — no hay nada que arreglar.");
                return Ok(());
            }
            Ok(Some(Msg::Failed { error })) => {
                println!("\n❌ DEPLOY FALLÓ: {error}");
                failed_err = Some(error);
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                println!("== canal cerrado en deploy");
                break;
            }
            Err(_) => {
                println!("== timeout (15 min) en deploy");
                break;
            }
        }
    }

    let (Some(id), Some(err)) = (sandbox_id, failed_err) else {
        return Err(anyhow!("no hubo un fallo con VM viva sobre el que actuar"));
    };

    // 3) El agente entra al ruedo sobre la VM viva.
    println!("\n========== EL AGENTE ENTRA AL RUEDO ==========\n");
    crate::agent::spawn_fix_agent(client.clone(), id.clone(), "agenda-e2e".into(), err, tx.clone());
    loop {
        match tokio::time::timeout(Duration::from_secs(600), rx.recv()).await {
            Ok(Some(Msg::AgentStep { text })) => println!("   {text}"),
            Ok(Some(Msg::Live { url })) => {
                println!("\n🟢🟢 EL AGENTE LO ARREGLÓ — LIVE: {url}");
                break;
            }
            Ok(Some(Msg::AgentDone { outcome })) => {
                println!("\n== AGENTE TERMINÓ: {outcome:?}");
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                println!("== canal cerrado en fase agente");
                break;
            }
            Err(_) => {
                println!("== timeout (10 min) en fase agente");
                break;
            }
        }
    }
    println!("\n== fin. VM {id} sigue viva (gestiónala en el panel del TUI).");
    Ok(())
}

/// Suelta el agente sobre una VM YA VIVA con un deploy fallido (sin re-deployar).
/// Útil para iterar el agente contra el mismo fallo. Uso:
///   cargo run -- --agent-fix sb_xxx   (inferencia vía EasyBits, sin key extra)
pub async fn agent_fix(sandbox_id: String) -> Result<()> {
    use crate::app::Msg;
    use std::time::Duration;
    use tokio::sync::mpsc;

    let client = build_client().await?;
    client.validate().await.map_err(|e| anyhow!("validación EasyBits falló: {e}"))?;

    let log = crate::app::fetch_app_log(&client, &sandbox_id).await;
    let err = crate::app::trim_log(&log);
    println!("== fallo capturado de {sandbox_id}:\n{err}\n");
    println!("========== EL AGENTE ENTRA AL RUEDO ==========\n");

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    crate::agent::spawn_fix_agent(client.clone(), sandbox_id.clone(), "agenda-e2e".into(), err, tx);
    loop {
        match tokio::time::timeout(Duration::from_secs(600), rx.recv()).await {
            Ok(Some(Msg::AgentStep { text })) => println!("   {text}"),
            Ok(Some(Msg::Live { url })) => {
                println!("\n🟢🟢 EL AGENTE LO ARREGLÓ — LIVE: {url}");
                break;
            }
            Ok(Some(Msg::AgentDone { outcome })) => {
                println!("\n== AGENTE TERMINÓ: {outcome:?}");
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                println!("== canal cerrado / timeout en fase agente");
                break;
            }
        }
    }
    Ok(())
}

/// Destruye una VM por id (cleanup de pruebas). Uso: cargo run -- --destroy sb_xxx
pub async fn destroy(sandbox_id: String) -> Result<()> {
    let client = build_client().await?;
    client.destroy(&sandbox_id).await?;
    println!("✓ VM {sandbox_id} destruida");
    Ok(())
}

/// Construye un `Client` de EasyBits desde `EASYBITS_API_KEY` o las credenciales OAuth.
async fn build_client() -> Result<Client> {
    if let Ok(k) = std::env::var("EASYBITS_API_KEY") {
        return Client::new(k);
    }
    let mut creds =
        crate::oauth::load_creds().ok_or_else(|| anyhow!("sin credenciales — conéctate en el TUI primero"))?;
    if creds.is_expired() {
        creds = crate::oauth::refresh(&creds)
            .await
            .map_err(|e| anyhow!("refresh de token falló: {e}"))?;
    }
    Client::new(creds.access_token)
}

fn step_label(s: crate::app::StepStatus) -> &'static str {
    use crate::app::StepStatus::*;
    match s {
        Pending => "·",
        Running => "▸",
        Done => "✓",
        Failed => "✗",
    }
}

fn trunc(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}
