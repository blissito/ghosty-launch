# Handoff — Agente de errores (estado 2026-06-16, sesión 2)

## Resumen de la sesión

Se rediseñó el loop del agente (de "improvisado y frágil" a robusto) y se cerró el
círculo de honestidad del deploy. Veredicto del spike confirmado: **el loop de ghostycode
NO es portable** (soldado al TUI); lo valioso ya está vendorizado en `loop-engine`. Así que
endurecimos NUESTRO loop (Opción A), no migramos.

## Lo que se arregló hoy (todo en `main`, compila, 16 tests verdes)

### Loop del agente (`src/agent.rs`, `src/app.rs`, `src/main.rs`)
- **`Esc` cancela SIEMPRE.** `App.agent_cancel: Arc<AtomicBool>` que el loop chequea entre
  pasos; `App::cancel_agent()` desbloquea un `need_secret` pendiente y suelta la UI al panel
  al instante. `spawn_fix_agent` respeta la cancelación (no publica Live por accidente).
- **Anti-bucle.** Mapa `asked: HashMap<String,bool>` (provista/declinada). `need_secret` no
  re-pide lo ya resuelto; si insiste, le dice "ya la tienes, sospecha del valor / termina".
- **Awareness de envs.** El agente siembra `asked` + el contexto inicial desde el override
  durable → ve lo ya configurado y no lo re-pide.
- **Pegar `.env` entero.** `need_secret` parsea un bloque multilínea `KEY=VALUE` (soporta
  `export`, comillas; distingue KEY válida de un valor base64 con `=`) y guarda TODAS las
  vars de una. El paste ya no borra newlines; el input muestra `[N variables pegadas]`.
- **`restart` robusto.** Antes `pkill -f node; sleep 1` dejaba el :3000 ocupado → "sigue
  500". Ahora mata por patrón + por `PORT` en environ con `-9` y ESPERA a que el puerto
  quede libre (LISTEN `0BB8`/`0A` en `/proc/net/tcp`) antes de relanzar.

### Honestidad del deploy ("dice que está en vivo y no lo está")
- **Verificación de URL pública real.** `app::public_ok(url)` hace GET HTTPS a la URL
  pública (rustls, 5 reintentos × 3s) — valida lo que ve el navegador, no el loopback.
  `app::finalize_live()` reemplaza los 3 puntos que cantaban Live (deploy, reconfigure,
  agente): manda `Msg::Live` solo si responde de verdad, si no `Msg::LiveUnverified`.
- **Estado honesto** en Live cuando no verifica: aviso amarillo *"corre, pero la URL pública
  no responde aún · proxy/TLS de EasyBits · pulsa r para reintentar"*, sin confetti, sin
  mandar al agente. Tecla `r` reintenta.
- **Pasos normalizados al llegar a Live** (`App::complete_steps()`): si el agente recuperó
  el deploy, los pasos congelados en un fallo ya no muestran "✗ … 50%" en la pantalla Live.

### Envs del deploy → override durable (`src/recipe.rs`, `src/app.rs`)
- **`recipe::persist_envs()`**: las envs que el usuario da al deploy/reconfigure se
  persisten (upsert) en el override durable. Antes solo vivían en el proceso → el agente
  quedaba ciego a ellas y las re-pedía. Ahora el deploy y el reconfigure las guardan donde
  el agente las ve. ESTE era el bug de raíz del "le doy todo pero me lo pide al reparar".

### Audio (`src/audio.rs`, `src/main.rs`)
- Fuera el drone ambiental continuo (cansaba). Ahora bips/blips discretos: `boot` al abrir,
  `start` al iniciar acción, `done` al terminar/borrar, `chime` (el favorito) al quedar Live
  de verdad. Tecla `m` silencia. Cada uno con fade-in (sin clicks).

### Debug
- **`--exec <sandbox_id> "<cmd>"`**: exec crudo en una VM viva. Con él diagnosticamos el
  fallo de la URL pública capa por capa.

## El hallazgo grande: el TLS del proxy de EasyBits

Diagnóstico confirmado en una VM viva: la app está sana (escucha `0.0.0.0:3000`, responde
200 incluso con el Host público), DNS resuelve, **pero el handshake TLS al dominio público
falla** (`curl (35): tlsv1 alert internal error`). El edge de EasyBits no sirve el cert para
`*-3000.sandboxes.easybits.cloud`. **Es lado-plataforma (EasyBits es de blissito), no de
launch.** Launch ahora lo reporta con honestidad (aviso amarillo) en vez de verde falso.

## Pendientes / follow-ups

1. **EasyBits TLS** (plataforma): por qué el edge no aprovisiona el cert de los hosts con
   `-<port>`. Es lo que rompe la URL pública en las pruebas.
2. **Paciencia del health check del deploy.** Apps que bootean lento (mailmask: SNS, AWS)
   exceden la ventana (~40-60s) → el deploy marca "no respondió" y dispara al agente sin
   necesidad. Considerar subir el presupuesto o hacerlo adaptativo. (El agente recupera, pero
   es un rodeo evitable.)
