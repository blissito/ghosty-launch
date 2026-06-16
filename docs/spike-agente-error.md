# Spike — Agente de errores de deploy ("launch es el agente")

Objetivo del spike: validar si `ghosty-launch` puede **embeber el arnés de ghostycode**
para correr un loop de agente (DeepSeek) que, ante un deploy fallido, diagnostica y
arregla — usando el VM como manos (`exec`) y aterrizando el fix en una receta durable.

Todo lo de abajo está verificado contra las firmas reales de `/Users/bliss/ghostycode`
(crates `tools`, `protocol`, `agent`, `core`), no supuesto.

---

## TL;DR — veredicto

- ❌ **"Meter el arnés completo" NO se puede barato.** El loop agéntico real (la llamada
  HTTP a DeepSeek, el parseo de `tool_calls`, `finish_reason`) vive **soldado al TUI** en
  `crates/tui/src/client.rs` (~2700 líneas). No es una librería. Embeberlo = embeber el TUI.
- ✅ **Lo valioso SÍ es embebible y pesa ~nada.** Los crates `ghosty-tools` +
  `ghosty-protocol` + `ghosty-agent` son la sustancia reutilizable: tipos de tool, registry
  con dispatch, y el registry de modelos con DeepSeek ya cableado. `ToolRegistry::register`
  es **público** → puedo inyectar tools propias **sin forkear**.
- ✅ **Recomendación: reusar la sustancia, escribir el loop yo** (~80 líneas, OpenAI-compat,
  `reqwest` ya es dep). Es lo que quiero de todos modos: DeepSeek necesita niñera y mis
  tools de receta durable son específicas de launch.

`go` con la arquitectura **Path A** de abajo.

---

## Hallazgo decisivo: dónde vive el loop

| Pieza | Dónde está | ¿Embebible como lib? |
|---|---|---|
| Llamada HTTP a DeepSeek + parseo `tool_calls` | `crates/tui/src/client.rs` | ❌ soldado al TUI |
| `Runtime::handle_prompt` | `crates/core` | resuelve modelo + registra msg — **NO** llama al modelo ni itera |
| `Runtime::invoke_tool` | `crates/core` | despacha UN tool — el loop lo manejas tú |
| `ToolRegistry` (register/dispatch) | `crates/tools` | ✅ **público y extensible** |
| Tipos `ToolSpec/ToolHandler/ToolPayload/ToolOutput/ToolKind` | `crates/tools` + `crates/protocol` | ✅ |
| `ModelRegistry` (DeepSeek default, base url, key) | `crates/agent` | ✅ |

Evidencia: `grep "chat/completions\|tool_calls\|finish_reason"` solo pega en
`crates/tui/src/client.rs`. `core/src/lib.rs:1007` (`handle_prompt`) resuelve el modelo y
emite eventos, pero nunca hace la request agéntica.

### Las dos capas de tools (no confundir)

- **`crates/tui/src/tools/` (`trait ToolSpec` con `execute`)** → sistema del TUI, privado,
  NO inyectable desde fuera. *(Esta es la que un primer mapeo confundió como "la" capa.)*
- **`crates/tools` (`struct ToolSpec` + `trait ToolHandler` + `ToolRegistry`)** → la que
  `Runtime` realmente consume. `register()` es `pub`. **Esta es la buena.**

---

## Arquitectura recomendada (Path A)

```
ghosty-launch (el agente)
│
├─ loop DeepSeek (propio, ~80 líneas)          [reqwest → /chat/completions]
│     messages → modelo → tool_calls → dispatch → reinyecta → repite
│
├─ ghosty-tools::ToolRegistry                  [reusado: register + dispatch]
│     ├─ VmTool "run_in_vm"    → easybits::Client.exec_background (VM = manos)
│     ├─ VmTool "read_app_log" → fetch_app_log
│     ├─ RecipeTool "set_env"  → escribe ghosty.toml (DURABLE)
│     ├─ RecipeTool "set_start"→ escribe ghosty.toml (DURABLE)
│     ├─ "need_secret"         → pausa, pide valor en la TUI
│     └─ "restart" / "finish"
│
├─ ghosty-agent::ModelRegistry                 [reusado: resuelve deepseek-v4-pro]
└─ ghosty.toml extendido                        [receta durable: build/start/envs/runtime]
```

Por qué Path A y no embeber `Runtime`: `Runtime` no me da el loop (está en el TUI), así
que tendría que escribir el loop igual — pero pagando el costo de construir `Runtime`
(StateStore, McpManager, ExecPolicy, Hooks) para casi nada. Path A toma solo lo que aporta.

---

## Código real

### 1. `Cargo.toml` — deps nuevas (path al monorepo de ghostycode)

```toml
[dependencies]
# ... lo existente (reqwest, serde, tokio, anyhow) ...
ghosty-tools    = { path = "../ghostycode/crates/tools" }
ghosty-protocol = { path = "../ghostycode/crates/protocol" }
ghosty-agent    = { path = "../ghostycode/crates/agent" }
async-trait     = "0.1"
```

> Nota: ghostycode es edition 2024 / rust 1.88+. `ghosty-launch` tendría que subir a esa
> toolchain. Riesgo bajo pero real — validar que el árbol de deps transitivo compila (ver
> "qué falta validar").

### 2. Una tool = `impl ToolHandler` que sostiene el `Client` de EasyBits

Las firmas reales (`crates/tools/src/lib.rs`, `crates/protocol/src/lib.rs`):

```rust
// ToolSpec { name, input_schema, output_schema, supports_parallel_tool_calls, timeout_ms }
// (OJO: ToolSpec NO tiene `description` — la description para el wire la pongo yo)
// trait ToolHandler { fn kind()->ToolKind; fn is_mutating()->bool; async fn handle(ToolInvocation)->Result<ToolOutput,FunctionCallError> }
// ToolPayload::Function { arguments: String }   <- JSON string de args
// ToolOutput::Function  { body: Option<Value>, success: bool }

use async_trait::async_trait;
use ghosty_protocol::{ToolKind, ToolOutput, ToolPayload};
use ghosty_tools::{FunctionCallError, ToolHandler, ToolInvocation, ToolSpec};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::easybits::Client;

/// Tool "run_in_vm" — corre un comando en la VM rota. El VM es el ruedo.
pub struct RunInVm {
    client: Client,
    sandbox_id: String,
}

#[async_trait]
impl ToolHandler for RunInVm {
    fn kind(&self) -> ToolKind { ToolKind::Function }
    fn is_mutating(&self) -> bool { true } // puede tocar el VM; OK, es desechable

    async fn handle(&self, inv: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let args: Value = match &inv.payload {
            ToolPayload::Function { arguments } => serde_json::from_str(arguments)
                .map_err(|e| FunctionCallError::ExecutionFailed {
                    name: inv.tool_name.clone(), error: e.to_string() })?,
            _ => return Err(FunctionCallError::KindMismatch {
                expected: ToolKind::Function, got: ToolKind::Mcp }),
        };
        let cmd = args["command"].as_str().unwrap_or_default();

        // Reusa el patrón exec_oneshot que YA existe en app.rs (exec_background + poll).
        let out = crate::app::exec_oneshot_pub(&self.client, &self.sandbox_id, cmd, 30)
            .await
            .ok_or_else(|| FunctionCallError::ExecutionFailed {
                name: inv.tool_name.clone(), error: "exec no terminó".into() })?;

        Ok(ToolOutput::Function {
            body: Some(json!({
                "stdout": out.stdout, "stderr": out.stderr, "exit_code": out.exit_code,
            })),
            success: out.exit_code == Some(0),
        })
    }
}
```

`RecipeTool "set_env"` es igual pero su `handle` escribe a `ghosty.toml` (durable) en vez de
tocar el VM. `finish` devuelve `ToolOutput::Function { body: {diagnosis, fixed}, success }` y
el loop corta al verlo.

### 3. Construir el registry (vacío + mis tools)

```rust
use ghosty_tools::ToolRegistry;

fn build_registry(client: &Client, id: &str) -> ToolRegistry {
    let mut reg = ToolRegistry::default(); // arranca vacío
    let spec = |name: &str, schema: Value| ToolSpec {
        name: name.into(), input_schema: schema,
        output_schema: json!({"type":"object"}),
        supports_parallel_tool_calls: false, timeout_ms: Some(90_000),
    };
    reg.register(
        spec("run_in_vm", json!({"type":"object",
            "properties":{"command":{"type":"string"}},"required":["command"]})),
        Arc::new(RunInVm { client: client.clone(), sandbox_id: id.into() }),
    ).unwrap();
    // ... set_env, set_start, read_app_log, need_secret, restart, finish ...
    reg
}
```

### 4. El loop (lo único que escribo de cero — OpenAI-compatible)

```rust
// Vía EasyBits proxy: base_url + auth los da el Client; aquí muestro la forma directa.
// DeepSeek: POST {base}/chat/completions, tools=[{type:"function",function:{name,description,parameters}}]
async fn fix_loop(reg: &ToolRegistry, http: &reqwest::Client, ctx: FailCtx) -> Result<Outcome> {
    let mut messages = vec![
        json!({"role":"system","content": SYSTEM_PROMPT}),
        json!({"role":"user","content": ctx.describe()}), // log + puerto + start actual
    ];
    let tools = wire_tools(); // ToolSpec.name + mi description + input_schema → schema OpenAI

    for _step in 0..12 { // presupuesto duro de pasos (caveat DeepSeek)
        let resp: Value = http.post(format!("{DEEPSEEK_BASE}/chat/completions"))
            .bearer_auth(&api_key)
            .json(&json!({"model":"deepseek-v4-pro","messages":messages,
                          "tools":tools,"tool_choice":"auto"}))
            .send().await?.json().await?;

        let msg = &resp["choices"][0]["message"];
        messages.push(msg.clone());

        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        if calls.is_empty() { continue; } // texto sin tool → re-pregunta o corta

        for c in calls {
            let name = c["function"]["name"].as_str().unwrap();
            let args = c["function"]["arguments"].as_str().unwrap().to_string();
            let call = ghosty_tools::ToolCall {
                name: name.into(),
                payload: ToolPayload::Function { arguments: args },
                source: ghosty_tools::ToolCallSource::Direct,
                raw_tool_call_id: c["id"].as_str().map(String::from),
            };
            let out = reg.dispatch(call, /*allow_mutating=*/ true).await
                .unwrap_or_else(|e| ToolOutput::Function {
                    body: Some(json!({"error": format!("{e:?}")})), success: false });

            if name == "finish" { return Ok(Outcome::from(out)); }
            messages.push(json!({"role":"tool","tool_call_id": c["id"],
                                 "content": serde_json::to_string(&out)?}));
        }
    }
    Ok(Outcome::Exhausted)
}
```

### 5. Wiring en launch (la tecla `a` en la pantalla Error)

- `app.rs`: nueva `Screen::Agent`, un `Msg::AgentStep { text }` (stream) y `Msg::AgentDone`.
- `main.rs` `Screen::Error`: agregar `KeyCode::Char('a') if app.sandbox_id.is_some()` →
  `spawn_fix_agent(client, id, ctx, tx)`.
- `spawn_fix_agent` = `tokio::spawn` que arma el registry + corre `fix_loop`, mandando cada
  paso a la UI (los ojos del fantasma en trance, que ya existen). Al terminar:
  re-`health_check` → verde: `expose` + `Live`; rojo: muestra qué hizo + logs.

Reusa lo ya construido: `exec_oneshot`, `fetch_app_log`, `health_check`, `spawn_reconfigure`.

---

## Qué queda por validar (antes de comprometer)

1. **Compila el árbol de deps de los 3 crates** dentro de launch (edition 2024, rust 1.88+;
   `ghosty-agent` arrastra menos, `ghosty-tools`/`protocol` hay que ver transitivos). → un
   `cargo add` + `cargo check` lo dice en minutos.
2. **`exec_oneshot` y `BgStatus` deben hacerse `pub`** (hoy son privados en `app.rs`).
3. **Robustez de tool-calling de DeepSeek**: confirmar que `tool_choice:"auto"` + el formato
   de mensajes `role:"tool"` funciona contra el endpoint (o vía proxy EasyBits). Probar con
   un caso real (el `MongooseServerSelectionError`).
4. **Receta durable**: define el `ghosty.toml` extendido primero (es donde aterrizan
   `set_env`/`set_start`). Sin él, las tools de receta no tienen destino → spike (A) aparte.

## Futuro (no para v1)

- **Compartir el arnés de verdad**: extraer el loop de `crates/tui/src/client.rs` a un crate
  `ghosty-loop` reutilizable. Ahí sí "launch y ghostycode comparten arnés" sin reescribir.
- **Fase 2 MCP**: exponer las VM tools como server MCP para que un ghostycode local las use.
