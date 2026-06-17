# Handoff — Agente de errores (estado 2026-06-16, retomar mañana)

## El veredicto del día (de blissito)

Vamos parchando bug tras bug en la UX del agente. **"Improvisar el loop será un problema"** —
y es correcto. El loop agéntico hand-rolled (`src/agent.rs`) tiene demasiados estados frágiles
(se queda pidiendo lo mismo, no deja avanzar, no hay escape). Mañana: **rediseñar el loop/UX
con principios, no más parches puntuales.**

Síntoma reportado hoy: *"me dice el problema y ya no me deja avanzar ni hacer nada"* — el agente
diagnostica pero el usuario queda **atascado** en la pantalla del Agente.

## Qué SÍ funciona (probado, en `main`)

- **Motor** (`crates/loop-engine`): inferencia DeepSeek v4-pro **vía EasyBits** (`/api/v2/llm/v1`,
  mismo bearer, sin key extra), con reasoning. 177 tests verdes. Sólido.
- **Auth**: `oauth::fresh_bearer()` refresca + **persiste** el token (el endpoint LLM rechaza
  tokens rancios). Arreglado.
- **Inyección de envs**: PROBADA — deploy de `agenda` con `DATABASE_URL` dummy → 🟢 Live. Las
  envs SÍ llegan a la app. (agenda solo necesita que `DATABASE_URL` *exista* para bootear.)
- **Receta durable** (`src/recipe.rs`): override local que el deploy fusiona sobre el repo.
- **Narrativa** del agente (habla en español, no comandos crudos) + **outcome card**.
- **Paste** en el prompt inline del secreto + Enter ignora vacío + Ctrl+U/W. Arreglado.
- **Audio** ambiental (drone + chime, tecla `m`, `GHOSTY_NO_AUDIO`).

## El problema central a resolver mañana: el loop/UX se atasca

Hipótesis (verificar con repro fresco + screenshot):

1. **No hay escape del agente.** En `Screen::Agent`, mientras `agent_busy` o `agent_prompt`,
   solo `q` (que cierra TODA la app) responde. Si el agente loopea/cuelga, el usuario queda
   atrapado. → **Falta: `Esc` cancela el agente en cualquier momento → vuelve a Error/Apps.**
   (Requiere poder abortar la tarea del agente: un `CancellationToken`/flag que el loop chequee.)
2. **Re-pregunta en bucle.** Si el usuario da `DATABASE_URL` pero la app sigue fallando (valor
   inválido, Mongo inalcanzable que SÍ bloquea, u otra causa), el agente vuelve a pedir lo mismo
   → sensación de "mismo problema". → El loop necesita: detectar que ya pidió X, no repetir;
   terminar con un estado claro tras N intentos.
3. **El agente no sabe qué envs YA configuró el usuario.** Diagnostica "falta DATABASE_URL"
   corriendo `env` en un shell nuevo, pero las envs inyectadas son **process-scoped** (solo en el
   proceso de la app, no en un shell nuevo). → Pasarle al agente la **lista de env keys ya
   configuradas** (de `spawn_launch`/override) para que distinga "falta" de "está pero el valor
   es malo".

## Decisión estratégica para mañana

**¿Seguir con el loop hand-rolled (endurecido) o cambiar de enfoque?**

- **Opción A — Endurecer el loop actual**: agregar (a) escape/cancel siempre disponible, (b)
  guardas anti-bucle (no re-pedir, límite de intentos, estados terminales claros), (c) awareness
  de envs ya configuradas, (d) timeouts. Es seguir en `src/agent.rs` pero con un diseño de
  máquina de estados explícita, no improvisado.
- **Opción B — Reconsiderar el enfoque**: ¿vale más constreñir el agente (menos autonomía, más
  determinismo) o apoyarse más en el loop probado de ghostycode? blissito sospecha que improvisar
  es frágil — evaluar si un loop más principista/constreñido evita este whack-a-mole.

Recomendación de arranque: empezar por el **escape garantizado** (bug #1, el más doloroso:
"no me deja avanzar"), luego decidir A vs B con un repro claro en mano.

## Primer paso concreto mañana

1. Repro con screenshot del estado "atascado" exacto (¿busy infinito? ¿pidiendo en bucle?
   ¿outcome sin acción?).
2. Implementar `Esc` = cancelar agente desde cualquier estado de `Screen::Agent`.
3. Con eso, decidir A (endurecer) vs B (reconsiderar enfoque).

## Archivos clave
- `src/agent.rs` — el loop + tools + dispatch (lo que hay que rediseñar).
- `src/main.rs` — `Screen::Agent` key handling (input inline, falta el escape).
- `src/app.rs` — estado del agente (`agent_busy/prompt/reply/outcome/pending`).
- `src/ui.rs` — `agent_screen()` render.
