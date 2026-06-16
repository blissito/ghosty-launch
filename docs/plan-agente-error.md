# Plan — Agente de errores de deploy

**Principio rector:** `ghostycode` es **solo lectura**. Copiamos (vendor) las piezas que
ocupemos a ESTE repo y las adaptamos. **No** hay crates compartidos ni dependencias `path`
a ghostycode. Launch es un proyecto separado y autocontenido.

**Qué construimos:** ante un deploy fallido, launch suelta un agente (DeepSeek) que
diagnostica desde el log real del VM y arregla aterrizando el fix en una **receta durable**
(`ghosty.toml`), luego reinicia y re-verifica.

---

## Componentes

1. **Motor LLM vendorizado** (lo complejo, copiado de ghostycode `client.rs` y recortado):
   `complete_turn()` con replay de `reasoning_content`, cache byte-idéntica, mapeo de
   `reasoning_effort`, HTTP/SSE + reintentos, parseo de `tool_calls`/`usage`. **Sin**
   compaction, **sin** threads. Vive en un crate local `crates/loop-engine` (workspace propio
   de launch).
2. **Registry de tools propio** (~50 líneas — NO vendorizamos `ghosty-tools`, es trivial):
   `HashMap<name, handler>` + `dispatch`.
3. **VM-tools** (`ToolHandler` que sostienen el `Client` de EasyBits): `run_in_vm`,
   `read_app_log`, `set_env`, `set_start`, `need_secret`, `restart`, `finish`.
4. **Driver wizard** (~80 líneas): el loop acotado (presupuesto de pasos, parser tolerante).
5. **Receta `ghosty.toml` extendida** (build+start+envs+runtime): la lee el deploy normal Y
   el agente; es donde aterriza el fix durable.
6. **UI**: `Screen::Agent`, tecla `a` en Error, stream de pasos (ojos del fantasma en
   trance), re-health-check → Live / diagnóstico.

---

## Fases (secuenciadas por dependencia)

### Fase 0 — De-risk (~0.5 día)
- [ ] Leer `crate::llm_client` en ghostycode: qué tipos arrastra el motor (probable `LlmError`
      + structs chicos). Define el alcance exacto del copy.
- [ ] `cargo check` de un esqueleto del motor vendorizado en la toolchain de launch.
      Ghostycode usa edition 2024 / rust 1.88 (`let_chains`); al copiar, **adaptamos** a la
      toolchain de launch (reescribir let-chains a `if` anidados si hace falta). No adoptamos
      su edition.
- [ ] Confirmar fuente de la key DeepSeek: endpoint proxy EasyBits **o** `DEEPSEEK_API_KEY`
      como fallback para v1.

### Fase 1 — Receta `ghosty.toml` extendida (~1 día) · *fundación*
- [ ] Schema: `build`, `start`, `envs`, `node_version`, `port`, `system_deps`.
- [ ] Helpers read/write en launch.
- [ ] El deploy normal (`app.rs:1317`) pasa a leer de la receta (fuente única).
- [ ] Sin esto el agente no tiene dónde dejar el fix → va primero.

### Fase 2 — Motor LLM vendorizado (~2-3 días) · *lo difícil*
- [ ] Crear `crates/loop-engine`. Copiar + recortar de `client.rs`:
      wire/reasoning/http/provider/usage.
- [ ] **Traer los ~20 tests de reasoning** como red de seguridad (validan el copy).
- [ ] `ModelWire` + config mínima (cortar la dependencia del `config.rs` de 10k líneas:
      solo el subset provider/retry).
- [ ] API pública: `async fn complete_turn(http, &TurnRequest) -> Result<AssistantTurn>`.

### Fase 3 — Registry + tools + driver (~1-2 días)
- [ ] Registry propio (`register` / `dispatch`).
- [ ] Hacer `pub` en `app.rs`: `exec_oneshot`, `fetch_app_log`, `health_check`, `BgStatus`.
- [ ] VM-tools: `run_in_vm`/`read_app_log` (→ VM), `set_env`/`set_start` (→ receta durable),
      `restart` (→ `spawn_reconfigure`), `need_secret`/`finish`.
- [ ] Driver: loop ≤12 pasos, `tool_choice:"auto"`, parser tolerante (texto sin tool →
      re-pregunta o corta).

### Fase 4 — UI wiring (~1 día)
- [ ] `Screen::Agent` + `Msg::AgentStep{text}` / `Msg::AgentDone`.
- [ ] `main.rs` Error: `KeyCode::Char('a') if sandbox_id.is_some()` → `spawn_fix_agent`.
- [ ] `spawn_fix_agent`: arma registry + corre driver, stream a la UI (ojos en trance).
- [ ] `need_secret` → pausa, pide el valor en la TUI, resume.
- [ ] Al terminar: re-`health_check` → verde: `expose`+`Live`; rojo: muestra qué hizo + logs.

### Fase 5 — Prueba real + pulido (~1 día)
- [ ] Caso `MongooseServerSelectionError` (env faltante) end-to-end:
      diagnostica → `set_env`(needs_secret) → pide valor → `restart` → verde.
- [ ] Tolerancia DeepSeek (presupuesto, malformed tool_calls).

**Total: ~6-9 días.**

---

## Dependencias / riesgos

- **Endpoint proxy EasyBits para DeepSeek** — si no existe aún, v1 corre con
  `DEEPSEEK_API_KEY` directo y el proxy queda para después.
- **Robustez tool-calling DeepSeek** — mitigado por: motor vendorizado (ya tuneado) +
  presupuesto de pasos + parser tolerante.
- **Adaptación de toolchain** del código copiado (edition 2024 → la de launch).

## Fuera de alcance (v1)
- Compaction / sesiones largas (launch es wizard, fix corto).
- Arreglar **código** del repo / abrir PR (v1 solo arregla config de deploy).
- Fase 2 MCP (exponer VM-tools a un ghostycode externo).
