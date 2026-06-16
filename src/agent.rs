//! El agente que "entra al ruedo" cuando un deploy falla.
//!
//! Launch ES el agente: este módulo corre un loop agéntico contra DeepSeek (motor
//! vendorizado en `loop-engine`, con todo el reasoning que pone a v4-pro en la liga de
//! Sonnet) usando el VM como manos (`exec`) y aterrizando el arreglo en el override
//! durable ([`crate::recipe`]). Modelo OpenHands: el cerebro vive aquí, el VM ejecuta.
//!
//! Flujo: investiga el log/VM → arregla (envs/start no-secretos en el override) →
//! reinicia → verifica. Si necesita un secreto que no puede saber (connection string),
//! termina pidiéndolo y launch reusa la pantalla Envs.

use loop_engine::client::DeepSeekClient;
use loop_engine::config::{ApiProvider, Config};
use loop_engine::llm_client::LlmClient;
use loop_engine::models::{ContentBlock, Message, MessageRequest, SystemPrompt, Tool};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::Msg;
use crate::easybits::Client;
use crate::recipe;

/// Cómo terminó el agente — lo consume la UI para decidir el siguiente paso.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Aplicó un arreglo (envs/start) y reinició. Launch debe re-verificar el health.
    Applied { summary: String },
    /// Necesita que el usuario provea estos envs (secretos que el agente no inventa).
    NeedEnvs { summary: String, keys: Vec<String> },
    /// No pudo arreglarlo (sin secreto, sin diagnóstico claro, o agotó pasos).
    GaveUp { summary: String },
}

// La llave de EasyBits que ghosty-launch ya pide INCLUYE inferencia (DeepSeek v4-pro)
// bajo el mismo bearer. El agente la usa vía `Client` — cero key extra.
const MODEL_DEFAULT: &str = "deepseek-v4-pro";
const MAX_STEPS: usize = 24;
/// Tope de la salida de tools que ve el agente. Generoso: `trim_log` (200 bytes) es para
/// la UI; un agente necesita ver archivos/outputs completos o desperdicia pasos releyendo.
const TOOL_OUT_MAX: usize = 6000;

/// Recorta salida larga conservando cabeza + cola (el agente ve inicio y final).
fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max * 2 / 3).collect();
    let tail: String = {
        let t: Vec<char> = s.chars().rev().take(max / 3).collect();
        t.into_iter().rev().collect()
    };
    format!("{head}\n…[recortado]…\n{tail}")
}

const SYSTEM: &str = "\
Eres el agente de Ghosty Launch. Una app acaba de fallar su deploy en una micro-VM y tú \
entras al ruedo a arreglarla. El proceso arrancó pero no responde en el puerto (PORT=3000). \
Tienes shell en la VM (que es desechable: arréglala sin miedo) y herramientas para fijar \
arreglos DURABLES.\n\n\
Cómo trabajas:\n\
1. Investiga: lee el log y revisa la VM (package.json, node -v, qué puerto bindeó) con run_in_vm.\n\
2. Diagnostica la causa REAL (típico: falta un env, start command equivocado, no escucha en PORT).\n\
3. Arregla lo que SÍ puedas: set_env para envs NO secretos (NODE_ENV, una URL pública, PORT), \
set_start si el comando de arranque está mal. Estos quedan durables (sobreviven al re-deploy).\n\
4. restart para reiniciar con tus cambios, y verifica con run_in_vm que ahora responde en :3000.\n\
5. finish cuando termines.\n\n\
REGLA DE ORO sobre secretos: si falta un valor que NO puedes saber (connection string de Mongo/\
Postgres, API keys, tokens), NO lo inventes y NO te rindas: usa need_secret para pedírselo al \
usuario en pantalla. Te lo devolverá (ya guardado como env durable); entonces haz restart y \
verifica que la app responde. Solo usa finish con needs_envs si el usuario NO lo provee.\n\n\
NARRA en español, breve, antes de cada acción: di qué vas a hacer y qué encontraste (una frase). \
Eso es lo que ve el usuario.\n\n\
EFICIENCIA (tienes pasos limitados): el output de las tools viene COMPLETO — no repitas un \
comando ya ejecutado. Para inspeccionar la app, prueba a correr su start a mano en la VM y LEE el \
error real antes de teorizar. En cuanto tengas la causa, APLICA el arreglo (set_env/set_start/\
need_secret) y restart; no sigas investigando de más. La app debe escuchar en 127.0.0.1:3000.";

/// Lanza el agente de arreglo en una tarea async. Emite `Msg::AgentStep` por paso y
/// `Msg::AgentDone` al terminar.
pub fn spawn_fix_agent(
    client: Client,
    sandbox_id: String,
    app_name: String,
    fail_error: String,
    tx: UnboundedSender<Msg>,
) {
    tokio::spawn(async move {
        let outcome = run(&client, &sandbox_id, &app_name, &fail_error, &tx).await;
        // Si el agente dice que aplicó un arreglo, cerramos el loop nosotros:
        // re-verificamos el health y, si responde, exponemos el puerto → Live.
        if let Outcome::Applied { .. } = &outcome {
            step(&tx, "🩺 verificando que ahora responda en :3000…");
            if crate::app::health_check(&client, &sandbox_id).await {
                match client.expose(&sandbox_id, crate::app::APP_PORT).await {
                    Ok(exp) => {
                        step(&tx, "🟢 responde — publicando");
                        let _ = tx.send(Msg::Live { url: exp.url });
                        return;
                    }
                    Err(e) => step(&tx, &format!("responde pero no se pudo exponer: {e}")),
                }
            } else {
                step(&tx, "⚠️ reinició pero sigue sin responder en :3000");
            }
        }
        let _ = tx.send(Msg::AgentDone { outcome });
    });
}

async fn run(
    client: &Client,
    id: &str,
    app_name: &str,
    fail_error: &str,
    tx: &UnboundedSender<Msg>,
) -> Outcome {
    // Inferencia vía EasyBits, endpoint /api/v2/llm/v1. Usamos un bearer FRESCO (el del
    // `Client` puede estar rancio y el endpoint LLM lo rechaza); fallback al del Client
    // cuando la auth es por eb_sk key (sin creds OAuth que refrescar).
    let model = MODEL_DEFAULT.to_string();
    let bearer = crate::oauth::fresh_bearer()
        .await
        .unwrap_or_else(|| client.bearer().to_string());
    let cfg = Config::for_endpoint(
        bearer,
        client.llm_base_url(),
        ApiProvider::Deepseek,
        model.clone(),
    );
    let llm = match DeepSeekClient::new(&cfg) {
        Ok(c) => c,
        Err(e) => {
            return Outcome::GaveUp {
                summary: format!("No se pudo crear el cliente de inferencia EasyBits: {e}"),
            };
        }
    };

    step(tx, "🔎 leyendo el log de la VM…");
    let log = crate::app::fetch_app_log(client, id).await;
    let log_tail = clip(&log, TOOL_OUT_MAX);

    let mut messages = vec![user_text(format!(
        "El deploy falló así:\n{fail_error}\n\nÚltimas líneas de /tmp/app.log:\n{log_tail}\n\n\
         Diagnostica y arregla. La app debe responder en 127.0.0.1:3000."
    ))];
    let tools = tools();

    for _ in 0..MAX_STEPS {
        let req = MessageRequest {
            model: model.clone(),
            messages: messages.clone(),
            max_tokens: 8192,
            system: Some(SystemPrompt::Text(SYSTEM.into())),
            tools: Some(tools.clone()),
            tool_choice: Some(json!({ "type": "auto" })),
            metadata: None,
            thinking: None,
            reasoning_effort: Some("high".into()),
            stream: Some(false),
            temperature: None,
            top_p: None,
        };

        let resp = match llm.create_message(req).await {
            Ok(r) => r,
            Err(e) => {
                return Outcome::GaveUp {
                    summary: format!("Error llamando a DeepSeek: {e}"),
                };
            }
        };

        // Preserva el turno del assistant TAL CUAL (incluye los bloques Thinking) para
        // que el replay de reasoning del motor mantenga caliente la prefix-cache.
        messages.push(Message {
            role: "assistant".into(),
            content: resp.content.clone(),
        });

        // Narrativa: mostramos lo que Ghosty DICE (sus bloques Text, en español), no su
        // pensamiento crudo ni los comandos. Los ojos en trance ya transmiten que piensa.
        for block in &resp.content {
            if let ContentBlock::Text { text, .. } = block {
                if !text.trim().is_empty() {
                    step(tx, text.trim());
                }
            }
        }

        // Junta las tool calls del turno.
        let calls: Vec<(String, String, Value)> = resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => Some((id.clone(), name.clone(), input.clone())),
                _ => None,
            })
            .collect();

        if calls.is_empty() {
            // Sin tools: o terminó en texto, o se quedó pensando. Lo empujamos una vez.
            messages.push(user_text(
                "Si ya está arreglada llama finish; si no, sigue usando las herramientas.".into(),
            ));
            continue;
        }

        let mut results = Vec::new();
        for (call_id, name, input) in calls {
            if name == "finish" {
                return finish_outcome(&input);
            }
            step(tx, &tool_label(&name, &input));
            let (content, is_error) = dispatch(client, id, app_name, &name, &input, tx).await;
            results.push(ContentBlock::ToolResult {
                tool_use_id: call_id,
                content,
                is_error: Some(is_error),
                content_blocks: None,
            });
        }
        messages.push(Message {
            role: "user".into(),
            content: results,
        });
    }

    Outcome::GaveUp {
        summary: "Agoté los pasos sin dejar la app verde. Revisa el log.".into(),
    }
}

/// Ejecuta una tool y devuelve `(contenido, es_error)`.
async fn dispatch(
    client: &Client,
    id: &str,
    app_name: &str,
    name: &str,
    input: &Value,
    tx: &UnboundedSender<Msg>,
) -> (String, bool) {
    match name {
        "need_secret" => {
            let key = input["key"].as_str().unwrap_or_default();
            let why = input["reason"].as_str().unwrap_or("");
            if !crate::app::is_env_key(key) {
                return (format!("clave inválida: {key:?}"), true);
            }
            // Pide el valor al usuario INLINE y espera su respuesta (back-channel).
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let prompt = if why.is_empty() {
                format!("Necesito {key}")
            } else {
                format!("Necesito {key} — {why}")
            };
            let _ = tx.send(Msg::AgentNeedInput { prompt, reply: reply_tx });
            match reply_rx.await {
                Ok(v) if !v.trim().is_empty() => {
                    // Lo guardamos durable de una vez (el agente luego hace restart).
                    let mut ovr = recipe::load(app_name);
                    ovr.envs.retain(|(k, _)| k != key);
                    ovr.envs.push((key.to_string(), v.trim().to_string()));
                    let _ = recipe::save(app_name, &ovr);
                    (format!("el usuario proveyó {key}; ya quedó guardado como env durable"), false)
                }
                _ => (format!("el usuario no proveyó {key}"), true),
            }
        }
        "run_in_vm" => {
            let cmd = input["command"].as_str().unwrap_or_default();
            if cmd.is_empty() {
                return ("falta 'command'".into(), true);
            }
            match crate::app::exec_oneshot(client, id, cmd, 30).await {
                Some(st) => {
                    let out = format!("exit={:?}\n{}{}", st.exit_code, st.stdout, st.stderr);
                    (clip(&out, TOOL_OUT_MAX), st.exit_code != Some(0))
                }
                None => ("el comando no terminó a tiempo".into(), true),
            }
        }
        "read_app_log" => {
            let log = crate::app::fetch_app_log(client, id).await;
            (clip(&log, TOOL_OUT_MAX), false)
        }
        "set_env" => {
            let key = input["key"].as_str().unwrap_or_default();
            let val = input["value"].as_str().unwrap_or_default();
            if !crate::app::is_env_key(key) {
                return (format!("clave de env inválida: {key:?}"), true);
            }
            let mut ovr = recipe::load(app_name);
            ovr.envs.retain(|(k, _)| k != key);
            ovr.envs.push((key.to_string(), val.to_string()));
            if recipe::save(app_name, &ovr) {
                (format!("env {key} guardado (durable)"), false)
            } else {
                ("no se pudo guardar el override".into(), true)
            }
        }
        "set_start" => {
            let cmd = input["command"].as_str().unwrap_or_default();
            if cmd.is_empty() {
                return ("falta 'command'".into(), true);
            }
            let mut ovr = recipe::load(app_name);
            ovr.start = Some(cmd.to_string());
            if recipe::save(app_name, &ovr) {
                (format!("start guardado (durable): {cmd}"), false)
            } else {
                ("no se pudo guardar el override".into(), true)
            }
        }
        "restart" => {
            let ovr = recipe::load(app_name);
            let cmd = restart_command(&ovr);
            match crate::app::exec_oneshot(client, id, &cmd, 30).await {
                Some(st) => {
                    let out = format!("{}{}", st.stdout, st.stderr);
                    (
                        format!("reiniciada (exit={:?})\n{}", st.exit_code, clip(&out, TOOL_OUT_MAX)),
                        st.exit_code != Some(0),
                    )
                }
                None => ("el reinicio no terminó a tiempo".into(), true),
            }
        }
        other => (format!("herramienta desconocida: {other}"), true),
    }
}

/// Comando de reinicio en la VM: detecta el workdir, mata el proceso viejo y relanza
/// el start con los envs durables del override + PORT. Espejo de `spawn_reconfigure`.
fn restart_command(ovr: &recipe::Override) -> String {
    let envs: String = ovr
        .envs
        .iter()
        .filter(|(k, v)| crate::app::is_env_key(k) && !v.is_empty())
        .map(|(k, v)| format!("{k}={} ", crate::app::sh_squote(v)))
        .collect();
    let start = ovr.start.clone().unwrap_or_else(|| "npm start".into());
    format!(
        "for d in /app/src /app; do [ -f \"$d/package.json\" ] && WD=\"$d\" && break; done; \
         [ -z \"$WD\" ] && {{ echo NO_APP; exit 1; }}; cd \"$WD\"; \
         pkill -f node 2>/dev/null; sleep 1; rm -f /tmp/app.log; \
         ({envs}PORT=3000 setsid nohup {start} > /tmp/app.log 2>&1 &); sleep 3; echo RESTARTED"
    )
}

fn finish_outcome(input: &Value) -> Outcome {
    let summary = input["summary"]
        .as_str()
        .unwrap_or("El agente terminó.")
        .to_string();
    let keys: Vec<String> = input["needs_envs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if !keys.is_empty() {
        Outcome::NeedEnvs { summary, keys }
    } else if input["applied"].as_bool().unwrap_or(false) {
        Outcome::Applied { summary }
    } else {
        Outcome::GaveUp { summary }
    }
}

fn tools() -> Vec<Tool> {
    let tool = |name: &str, description: &str, schema: Value| Tool {
        tool_type: None,
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        allowed_callers: None,
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    };
    vec![
        tool(
            "run_in_vm",
            "Ejecuta un comando shell en la VM y devuelve stdout/stderr/exit. Úsalo para \
             investigar (cat log, node -v, leer package.json) y para verificar el puerto.",
            json!({"type":"object","properties":{
                "command":{"type":"string","description":"comando shell a ejecutar en la VM"}},
                "required":["command"]}),
        ),
        tool(
            "read_app_log",
            "Devuelve las últimas líneas de /tmp/app.log (stdout/stderr de la app).",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "set_env",
            "Fija un env DURABLE (sobrevive al re-deploy). Solo para valores NO secretos. \
             Para secretos usa finish con needs_envs.",
            json!({"type":"object","properties":{
                "key":{"type":"string"},"value":{"type":"string"}},
                "required":["key","value"]}),
        ),
        tool(
            "set_start",
            "Fija el comando de arranque DURABLE (ej. 'node dist/server.js').",
            json!({"type":"object","properties":{
                "command":{"type":"string"}},"required":["command"]}),
        ),
        tool(
            "restart",
            "Reinicia la app en la VM con los envs/start durables y la deja escuchando en :3000.",
            json!({"type":"object","properties":{}}),
        ),
        tool(
            "need_secret",
            "Pídele al USUARIO un valor secreto que no puedes saber (connection string, API \
             key, token). Se lo preguntamos en pantalla y te devolvemos lo que teclee, ya \
             guardado como env durable. Después usa restart para aplicarlo y verifica.",
            json!({"type":"object","properties":{
                "key":{"type":"string","description":"nombre del env, ej. DATABASE_URL"},
                "reason":{"type":"string","description":"por qué lo necesitas, una frase"}},
                "required":["key"]}),
        ),
        tool(
            "finish",
            "Termina. Si aplicaste un arreglo y la app responde, applied=true. Si necesitas \
             que el usuario provea secretos, lista sus claves en needs_envs.",
            json!({"type":"object","properties":{
                "summary":{"type":"string","description":"diagnóstico y qué hiciste, en una línea o dos"},
                "applied":{"type":"boolean"},
                "needs_envs":{"type":"array","items":{"type":"string"}}},
                "required":["summary"]}),
        ),
    ]
}

fn user_text(text: String) -> Message {
    Message {
        role: "user".into(),
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    }
}

/// Etiqueta humana de la acción (no el comando crudo) — la narrativa la lleva el Text.
fn tool_label(name: &str, input: &Value) -> String {
    match name {
        "run_in_vm" => "   · revisando la VM…".into(),
        "read_app_log" => "   · releyendo el log…".into(),
        "set_env" => format!("   · fijando {} (durable)", input["key"].as_str().unwrap_or("")),
        "set_start" => "   · ajustando el comando de arranque".into(),
        "restart" => "   · reiniciando la app…".into(),
        "need_secret" => "   · te pido un dato…".into(),
        other => format!("   · {other}"),
    }
}

fn step(tx: &UnboundedSender<Msg>, text: &str) {
    let _ = tx.send(Msg::AgentStep {
        text: text.to_string(),
    });
}
