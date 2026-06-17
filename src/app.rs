//! Estado del TUI (máquina de estados determinista) + orquestación del launch.

use crate::easybits::Client;
use crate::oauth::{self, Creds};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

/// Repo de referencia a desplegar. Override con GHOSTY_REF_REPO.
/// Default: un server Node trivial (cero build) para probar el pipeline.
pub const DEFAULT_REF_REPO: &str = "https://github.com/blissito/ghosty-ref-node.git";
/// Puerto donde escucha la app dentro de la VM.
pub const APP_PORT: u16 = 3000;
/// Template genérico Node + persistente.
pub const TEMPLATE: &str = "node";

/// Paleta de acentos para personalizar (nombre, RGB). El hex se inyecta a la app.
pub const ACCENTS: [(&str, (u8, u8, u8)); 5] = [
    ("morado", (167, 139, 250)),
    ("verde", (122, 211, 161)),
    ("rosa", (244, 114, 182)),
    ("ámbar", (245, 196, 83)),
    ("cian", (45, 212, 191)),
];

pub fn accent_hex(idx: usize) -> String {
    let (_, (r, g, b)) = ACCENTS[idx % ACCENTS.len()];
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Índice de la opción "custom" (después de los presets).
pub const CUSTOM_ACCENT: usize = ACCENTS.len();

/// Pasos enfocables en Customize (el último es el botón Publicar).
pub const FOCUS_NAME: u8 = 0;
pub const FOCUS_COLOR: u8 = 1;
pub const FOCUS_LOGO: u8 = 2;
pub const FOCUS_PUBLISH: u8 = 3;
pub const FOCUS_COUNT: u8 = 4;

/// Normaliza un hex tipo `#rrggbb` (o `rrggbb`); None si es inválido.
pub fn normalize_hex(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", h.to_lowercase()))
    } else {
        None
    }
}

/// El hex del acento elegido (preset o custom validado).
pub fn chosen_accent(app: &App) -> String {
    if app.accent_idx == CUSTOM_ACCENT {
        normalize_hex(&app.custom_hex).unwrap_or_else(|| accent_hex(0))
    } else {
        accent_hex(app.accent_idx)
    }
}

/// Próxima expresión de ojos (par izq/der) + cuántos ticks dura. Pseudo-random
/// determinista del tick (no es un ciclo fijo): neutral domina, a veces mira a un
/// lado/arriba/abajo, feliz, parpadeo corto, y raro un bizco (ojos descruzados).
fn next_eyes(tick: u64) -> ((&'static str, &'static str), u64) {
    let h = tick.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(13) ^ (tick << 7);
    let pick = h % 100;
    let jitter = h % 10;
    // Duraciones calmadas (poll ~50ms): neutral domina y dura 2-3.5s; las
    // expresiones se quedan ~1.2-1.8s. Cambios poco frecuentes, no jittery.
    match pick {
        0..=55 => (("●", "●"), 40 + jitter * 4), // neutral (largo)
        56..=65 => (("◑", "◑"), 24 + jitter),    // mira derecha
        66..=75 => (("◐", "◐"), 24 + jitter),    // mira izquierda
        76..=82 => (("◓", "◓"), 22 + jitter),    // arriba
        83..=88 => (("◒", "◒"), 22 + jitter),    // abajo
        89..=94 => (("◕", "◕"), 26 + jitter),    // feliz
        95..=97 => (("◐", "◑"), 14 + jitter),    // bizco (raro)
        _ => (("─", "─"), 2),                    // parpadeo
    }
}

/// Ojos para el estado "trabajando" (pantalla Launching). Ghosty se concentra con
/// los ojos cerrados (ambos) y de rato en rato entra en trance: los ojos giran como
/// hipnotizados antes de volver a cerrarse. Determinista del tick (no usa el idle).
pub fn working_eyes(tick: u64) -> (&'static str, &'static str) {
    // Vórtice hipnótico: anillos que laten hacia dentro/fuera (◌ ◍ ◎ ◉ ◎ ◍).
    // Paso cada 2 ticks (~100ms). Los dos ojos van medio ciclo desfasados → uno se
    // abre mientras el otro se cierra: mirada en trance, un poco loca.
    const VORTEX: [&str; 6] = ["◌", "◍", "◎", "◉", "◎", "◍"];
    let cycle = tick % 80;
    match cycle {
        // Concentrado: ojos cerrados un buen rato.
        0..=43 => ("─", "─"),
        // Transición: medio cerrados, "despertando" al trance.
        44..=47 => ("◡", "◡"),
        // Trance: el vórtice late, ojos desfasados.
        _ => {
            let l = VORTEX[(tick / 2 % 6) as usize];
            let r = VORTEX[((tick / 2 + 3) % 6) as usize];
            (l, r)
        }
    }
}

/// Borra la última palabra de un buffer (Ctrl+W). En URLs sin espacios = limpia todo.
pub fn delete_last_word(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
    while let Some(c) = s.chars().last() {
        if c == ' ' {
            break;
        }
        s.pop();
    }
}

/// Sanea el nombre para inyectarlo seguro en un comando shell (single-quoted).
fn safe_name(name: &str) -> String {
    let n: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .take(40)
        .collect();
    let n = n.trim().to_string();
    if n.is_empty() {
        "Mi app".to_string()
    } else {
        n
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    KeyEntry,
    Apps,   // panel: lista de tus apps publicadas (CRUD)
    Create, // pega la URL del repo a publicar
    Consent,
    Customize,
    Envs, // variables de entorno a inyectar (auto-cargadas de .env)
    Launching,
    Live,
    Logs,  // visor de /tmp/app.log de la VM
    Agent, // el agente arreglando el deploy fallido (stream de pasos)
    Error,
}

/// ¿Es `s` una clave de env válida para shell? `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_env_key(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Envuelve un valor en comillas simples a prueba de shell (cualquier `'`
/// interno se cierra/escapa/reabre: `'\''`).
pub fn sh_squote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Parsea contenido tipo dotenv → pares (clave, valor). Ignora líneas vacías y
/// comentarios (`#`), tolera `export KEY=…` y quita comillas que envuelvan el valor.
pub fn parse_dotenv(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_string();
        if !is_env_key(&key) {
            continue;
        }
        let mut val = v.trim();
        // Quita un par de comillas envolventes (dobles o simples).
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = &val[1..val.len() - 1];
        }
        // Última definición gana (igual que dotenv).
        out.retain(|(ek, _): &(String, String)| ek != &key);
        out.push((key, val.to_string()));
    }
    out
}

/// Lee `.env` del directorio de trabajo actual; vacío si no existe o falla.
pub fn load_dotenv() -> Vec<(String, String)> {
    std::fs::read_to_string(".env")
        .map(|c| parse_dotenv(&c))
        .unwrap_or_default()
}

/// Prefijo en el nombre de la VM que marca "esta app la publicó Ghosty Launch".
/// El sufijo es el nombre que el user le puso (para mostrar varias distintas).
pub const APP_PREFIX: &str = "gl:";

pub fn marker_name(app_name: &str) -> String {
    format!("{APP_PREFIX}{}", safe_name(app_name))
}

pub fn display_name(sandbox_name: &Option<String>) -> String {
    sandbox_name
        .as_deref()
        .and_then(|n| n.strip_prefix(APP_PREFIX))
        .unwrap_or("app")
        .to_string()
}

/// Una app publicada (fila del panel).
#[derive(Clone)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub url: String,
    pub running: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
}

pub struct Step {
    pub label: String,
    pub status: StepStatus,
    pub detail: String,
}

impl Step {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            status: StepStatus::Pending,
            detail: String::new(),
        }
    }
}

/// Mensajes que las tareas async mandan al loop de UI.
pub enum Msg {
    ValidateFailed {
        error: String,
    },
    AuthStatus {
        text: String,
    },
    /// Tras autenticar (o refrescar): lista de apps publicadas → panel.
    AppsLoaded {
        client: Client,
        email: Option<String>,
        apps: Vec<AppEntry>,
    },
    /// Crear: el repo es estático → empezar publicación al CDN.
    StaticStart,
    /// Crear: el repo es una app → ir a Consent (VM), con el repo elegido.
    AppCreate {
        repo: String,
    },
    SandboxCreated {
        id: String,
    },
    Step {
        idx: usize,
        status: StepStatus,
        detail: String,
    },
    Live {
        url: String,
    },
    /// La app corre y se expuso, pero la URL PÚBLICA no respondió (TLS/proxy de EasyBits,
    /// propagación). No es un fallo de la app — estado honesto, no verde falso.
    LiveUnverified {
        url: String,
    },
    Failed {
        error: String,
    },
    /// Logs de la VM traídos (a la vista de Logs o tras un fallo).
    Logs {
        text: String,
    },
    /// No se pudo leer la lista de apps (red/API). NO vacía el panel: distinto de
    /// "no hay apps". Evita que un fallo transitorio parezca "se borraron todas".
    AppsError {
        error: String,
    },
    /// Un paso del agente de arreglo (lo que piensa/hace) → se muestra en vivo.
    AgentStep {
        text: String,
    },
    /// El agente terminó: aplicó algo, pide envs, o se rindió.
    AgentDone {
        outcome: crate::agent::Outcome,
    },
    /// El agente necesita un valor del usuario (p.ej. un secreto) AHORA, sin salir de la
    /// pantalla. `reply` es el canal de vuelta: la UI manda el valor tecleado.
    AgentNeedInput {
        prompt: String,
        reply: tokio::sync::oneshot::Sender<String>,
    },
}

pub struct App {
    pub screen: Screen,
    pub tick: u64,
    pub key_input: String,
    pub email: Option<String>,
    pub client: Option<Client>,
    pub validating: bool,
    /// En la pantalla de auth: false = elegir (OAuth/llave), true = pegar llave.
    pub paste_mode: bool,
    /// OAuth/reconexión en curso (muestra spinner + auth_status).
    pub auth_busy: bool,
    pub auth_status: String,
    pub steps: Vec<Step>,
    pub url: Option<String>,
    pub sandbox_id: Option<String>,
    /// tick en que se publicó (fresh) → dispara el confetti. None = ver existente.
    pub live_at: Option<u64>,
    /// Ojos del fantasma (izq, der) + cuándo cambian + última actividad (para dormir).
    pub eyes: (&'static str, &'static str),
    pub eyes_until: u64,
    pub last_activity: u64,
    /// Panel de apps publicadas + cursor de selección.
    pub apps: Vec<AppEntry>,
    pub apps_cursor: usize,
    /// Pidiendo confirmación antes de destruir (en panel o Live).
    pub confirm_destroy: bool,
    /// Operación async en curso en el panel (borrando/actualizando) → spinner.
    pub busy: Option<String>,
    /// Personalización elegida en la pantalla Customize.
    pub app_name: String,
    pub accent_idx: usize,
    /// Foco en Customize: 0 = nombre, 1 = color, 2 = logo.
    pub focus: u8,
    /// Hex tecleado cuando el acento es "custom".
    pub custom_hex: String,
    /// Ruta local o URL del logo (drag&drop pega la ruta).
    pub logo_input: String,
    /// Variables de entorno a inyectar a la app (auto-cargadas de `.env`, editables).
    pub envs: Vec<(String, String)>,
    /// Buffer de la pantalla Envs: se teclea `CLAVE=valor` y Enter lo agrega.
    pub env_input: String,
    /// Logs de la app traídos de la VM (`/tmp/app.log`). None = no cargados aún.
    pub logs: Option<String>,
    /// Pantalla a la que volver desde la vista de Logs.
    pub logs_return: Screen,
    /// Índice del paso que corre ahora (para animar la barra de progreso).
    pub running_idx: Option<usize>,
    /// Tick en que empezó el paso en curso (ancla del "creep" de la barra).
    pub step_anchor: u64,
    /// Si Some(id): la pantalla Envs reconfigura esa VM existente (reinicia con las
    /// nuevas envs, sin reclonar). None = deploy fresco normal.
    pub reconfig_id: Option<String>,
    /// URL del repo a publicar (pantalla Create).
    pub repo_input: String,
    pub error: Option<String>,
    /// Pasos del agente de arreglo (lo que va pensando/haciendo), en vivo.
    pub agent_steps: Vec<String>,
    /// El agente sigue corriendo (mientras true, no aceptamos teclas de salida).
    pub agent_busy: bool,
    /// Resultado del agente cuando termina (decide el footer/acciones de la pantalla).
    pub agent_outcome: Option<crate::agent::Outcome>,
    /// Cuando un deploy falla con VM viva, dejamos aquí `(id, error)` para que el loop
    /// de `main` arranque el agente automáticamente (apply no puede spawnear tareas).
    pub agent_pending: Option<(String, String)>,
    /// Si Some: el agente está esperando que el usuario teclee un valor (el prompt).
    pub agent_prompt: Option<String>,
    /// Buffer de lo que el usuario teclea para el agente (el secreto).
    pub agent_input: String,
    /// Canal de vuelta hacia la tarea del agente con el valor tecleado.
    pub agent_reply: Option<tokio::sync::oneshot::Sender<String>>,
    /// Bandera de cancelación: `Esc` la prende y el loop del agente la chequea entre
    /// pasos para abortar. Compartida con la tarea async (de ahí el `Arc<AtomicBool>`).
    pub agent_cancel: Option<Arc<AtomicBool>>,
    /// Audio silenciado (tecla `m`).
    pub muted: bool,
    /// ¿La URL pública respondió de verdad? Falso = corre pero el proxy/TLS no sirve aún.
    pub public_verified: bool,
    /// Reintento de verificación pública en curso (tecla `r` en Live no verificado).
    pub public_checking: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            screen: Screen::KeyEntry,
            tick: 0,
            key_input: String::new(),
            email: None,
            client: None,
            validating: false,
            paste_mode: false,
            auth_busy: false,
            auth_status: String::new(),
            steps: Vec::new(),
            url: None,
            sandbox_id: None,
            live_at: None,
            eyes: ("●", "●"),
            eyes_until: 0,
            last_activity: 0,
            apps: Vec::new(),
            apps_cursor: 0,
            confirm_destroy: false,
            busy: None,
            app_name: String::new(),
            accent_idx: 0,
            focus: 0,
            custom_hex: "#".to_string(),
            logo_input: String::new(),
            envs: Vec::new(),
            env_input: String::new(),
            logs: None,
            logs_return: Screen::Live,
            running_idx: None,
            step_anchor: 0,
            reconfig_id: None,
            repo_input: String::new(),
            error: None,
            agent_steps: Vec::new(),
            agent_busy: false,
            agent_outcome: None,
            agent_pending: None,
            agent_prompt: None,
            agent_input: String::new(),
            agent_reply: None,
            agent_cancel: None,
            muted: false,
            public_verified: false,
            public_checking: false,
            should_quit: false,
        }
    }

    pub fn ref_repo() -> String {
        std::env::var("GHOSTY_REF_REPO").unwrap_or_else(|_| DEFAULT_REF_REPO.to_string())
    }

    /// Cerrar sesión: borra credenciales y vuelve a estado fresco (pantalla de conexión).
    pub fn logout(&mut self) {
        oauth::clear_creds();
        *self = App::new();
    }

    /// El buffer de texto editable según la pantalla/foco actual (para Ctrl+U/W).
    pub fn active_input(&mut self) -> Option<&mut String> {
        match self.screen {
            Screen::KeyEntry if self.paste_mode => Some(&mut self.key_input),
            Screen::Create => Some(&mut self.repo_input),
            Screen::Envs => Some(&mut self.env_input),
            Screen::Customize => match self.focus {
                FOCUS_NAME => Some(&mut self.key_input),
                FOCUS_LOGO => Some(&mut self.logo_input),
                FOCUS_COLOR if self.accent_idx == CUSTOM_ACCENT => Some(&mut self.custom_hex),
                _ => None,
            },
            // El agente pidiendo un secreto: el input activo es agent_input.
            Screen::Agent if self.agent_prompt.is_some() => Some(&mut self.agent_input),
            _ => None,
        }
    }

    /// Descarta el error y limpia el estado del flujo (steps/busy/confirm).
    /// Devuelve true si hay sesión (el caller recarga el panel); false → al inicio.
    pub fn dismiss_error(&mut self) -> bool {
        self.error = None;
        self.steps.clear();
        self.busy = None;
        self.confirm_destroy = false;
        self.url = None;
        self.sandbox_id = None;
        self.live_at = None;
        if self.client.is_some() {
            self.busy = Some("cargando…".into());
            self.screen = Screen::Apps;
            true
        } else {
            self.screen = Screen::KeyEntry;
            false
        }
    }

    /// Empezar a crear: limpia la personalización y pide el repo (prefilla el default).
    pub fn start_create(&mut self) {
        self.key_input.clear();
        self.logo_input.clear();
        self.accent_idx = 0;
        self.custom_hex = "#".into();
        self.focus = 0;
        self.error = None;
        // Pre-carga las envs del `.env` local (si lo hay); el user las edita luego.
        self.envs = load_dotenv();
        self.env_input.clear();
        self.repo_input = App::ref_repo();
        self.screen = Screen::Create;
    }

    /// Pasa a la pantalla de variables de entorno (tras personalizar) en modo
    /// deploy fresco (no reconfiguración).
    pub fn start_envs(&mut self) {
        self.reconfig_id = None;
        self.env_input.clear();
        self.screen = Screen::Envs;
    }

    /// Abre la pantalla de Envs para RECONFIGURAR una app existente: pre-carga el
    /// `.env` local y marca la VM a reiniciar. Al confirmar se reinicia en sitio.
    pub fn start_reconfigure_envs(&mut self, id: String) {
        self.sandbox_id = Some(id.clone()); // VM viva → el agente la puede arreglar si falla
        self.reconfig_id = Some(id);
        self.envs = load_dotenv();
        self.env_input.clear();
        self.error = None;
        self.screen = Screen::Envs;
    }

    /// Prepara los pasos para reiniciar una app en su VM (sin reclonar).
    pub fn start_reconfigure(&mut self) {
        self.logs = None;
        self.running_idx = None;
        self.step_anchor = self.tick;
        self.url = None;
        self.live_at = None;
        self.confirm_destroy = false;
        // No conocemos el repo de una app existente → que Live no muestre uno viejo.
        self.repo_input.clear();
        self.steps = vec![
            Step::new("Deteniendo la app"),
            Step::new("Reiniciando con tus variables"),
            Step::new("Verificando que responda"),
        ];
        self.screen = Screen::Launching;
    }

    /// Aplica `CLAVE=valor` a la lista: agrega/actualiza; con valor vacío elimina
    /// la clave. Ignora entradas con clave inválida. Devuelve true si tocó algo.
    pub fn upsert_env(&mut self, line: &str) -> bool {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else {
            return false;
        };
        let key = k.trim().to_string();
        if !is_env_key(&key) {
            return false;
        }
        let val = v.trim().to_string();
        self.envs.retain(|(ek, _)| ek != &key);
        if !val.is_empty() {
            self.envs.push((key, val));
        }
        true
    }

    /// Avanza la animación de ojos (llamar cada frame). Idle = expresiones random
    /// (mira a los lados, feliz, parpadeo, a veces bizco). Sin actividad ~10s se
    /// adormila (ojos medio cerrados) y ~18s se duerme del todo (cerrados).
    pub fn tick_eyes(&mut self) {
        const DROWSY_AFTER: u64 = 200; // ~10s
        const SLEEP_AFTER: u64 = 360; // ~18s
        let idle = self.tick.saturating_sub(self.last_activity);
        if idle > SLEEP_AFTER {
            self.eyes = ("─", "─"); // dormido
            return;
        }
        if idle > DROWSY_AFTER {
            self.eyes = ("◡", "◡"); // adormilado (medio cerrados)
            return;
        }
        if self.tick < self.eyes_until {
            return;
        }
        let (eyes, dur) = next_eyes(self.tick);
        self.eyes = eyes;
        self.eyes_until = self.tick + dur;
    }

    /// Ratio (0..1) de la barra de progreso del deploy. Cada paso ocupa una banda
    /// igual; los pasos terminados llenan su banda completa y el paso en curso
    /// "repta" suave dentro de la suya (asíntota a ~0.9 de la banda) según el tiempo
    /// transcurrido — así no se ve congelada durante el build largo. Con todos los
    /// pasos hechos da 1.0 (la barra se dibuja al 100% al quedar en vivo).
    pub fn launch_ratio(&self) -> f64 {
        let total = self.steps.len();
        if total == 0 {
            return 0.0;
        }
        let band = 1.0 / total as f64;
        let done = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .count();
        let mut ratio = done as f64 * band;
        if self.steps.iter().any(|s| s.status == StepStatus::Running) {
            // Constante de tiempo ~30s (tick ~20/s): sigue avanzando visiblemente
            // durante un build de minutos sin llegar nunca al borde de la banda.
            let elapsed = self.tick.saturating_sub(self.step_anchor) as f64;
            let creep = band * 0.9 * (1.0 - (-elapsed / 600.0).exp());
            ratio += creep;
        }
        ratio.min(1.0)
    }

    /// Aplica un mensaje de tarea async al estado.
    pub fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::ValidateFailed { error } => {
                self.validating = false;
                self.auth_busy = false;
                self.error = Some(error);
            }
            Msg::AuthStatus { text } => {
                self.auth_busy = true;
                self.error = None;
                self.auth_status = text;
            }
            Msg::AppsLoaded {
                client,
                email,
                apps,
            } => {
                self.auth_busy = false;
                self.validating = false;
                self.busy = None;
                self.client = Some(client);
                self.email = email;
                self.apps_cursor = self.apps_cursor.min(apps.len().saturating_sub(1));
                let empty = apps.is_empty();
                self.apps = apps;
                if empty {
                    // Sin apps: el panel sobra → directo a crear (pega el repo).
                    self.start_create();
                } else {
                    self.screen = Screen::Apps;
                }
            }
            Msg::StaticStart => {
                self.busy = None;
                self.steps = vec![
                    Step::new("Clonando el repo"),
                    Step::new("Creando sitio en el CDN"),
                    Step::new("Subiendo archivos"),
                ];
                self.screen = Screen::Launching;
            }
            Msg::AppCreate { repo } => {
                self.busy = None;
                self.repo_input = repo;
                self.screen = Screen::Consent;
            }
            Msg::SandboxCreated { id } => {
                self.sandbox_id = Some(id);
            }
            Msg::Step {
                idx,
                status,
                detail,
            } => {
                // Al ARRANCAR un paso nuevo, ancla el tick para animar la barra desde
                // 0 dentro de su banda. (Los polls repiten Running del mismo idx → no
                // reanclar, si no la barra se reiniciaría en cada poll.)
                if status == StepStatus::Running && self.running_idx != Some(idx) {
                    self.running_idx = Some(idx);
                    self.step_anchor = self.tick;
                }
                if let Some(s) = self.steps.get_mut(idx) {
                    s.status = status;
                    if !detail.is_empty() {
                        s.detail = detail;
                    }
                }
            }
            Msg::Live { url } => {
                self.url = Some(url);
                self.live_at = Some(self.tick); // publicación fresca → confetti
                self.public_verified = true;
                self.public_checking = false;
                // Si llegamos vía el agente, los pasos del deploy pueden haber quedado
                // congelados en un fallo (health check impaciente). Ya está en vivo →
                // normaliza la barra a 100% para no mostrar el cadáver del deploy.
                self.complete_steps();
                self.screen = Screen::Live;
            }
            Msg::LiveUnverified { url } => {
                // Corre, pero la URL pública no respondió. Honesto: sin confetti, con
                // aviso + reintento. No mandamos al agente (no es un fallo de la app).
                self.url = Some(url);
                self.live_at = None;
                self.public_verified = false;
                self.public_checking = false;
                self.complete_steps();
                self.screen = Screen::Live;
            }
            Msg::Failed { error } => {
                // Con VM viva el agente entra al ruedo AUTOMÁTICAMENTE (lo que esperas
                // por default). Sin VM (fallo de infra) no hay nada que arreglar → Error.
                if let Some(id) = self.sandbox_id.clone() {
                    self.start_fix_agent();
                    self.agent_pending = Some((id, error));
                } else {
                    self.error = Some(error);
                    self.screen = Screen::Error;
                }
            }
            Msg::Logs { text } => {
                self.busy = None;
                self.logs = Some(text);
            }
            Msg::AppsError { error } => {
                self.busy = None;
                // No tocamos self.apps: el panel se queda como estaba.
                self.error = Some(format!("No se pudo leer tus apps: {error}"));
                self.screen = Screen::Error;
            }
            Msg::AgentStep { text } => {
                self.agent_steps.push(text);
            }
            Msg::AgentDone { outcome } => {
                // Si el usuario ya canceló (Esc), ignoramos el outcome tardío: la UI ya
                // se soltó al panel y no queremos reabrir la pantalla del agente.
                let cancelled = self
                    .agent_cancel
                    .as_ref()
                    .is_some_and(|f| f.load(Ordering::Relaxed));
                self.agent_cancel = None;
                if cancelled {
                    return;
                }
                self.agent_busy = false;
                self.agent_prompt = None;
                self.agent_reply = None;
                self.agent_outcome = Some(outcome);
            }
            Msg::AgentNeedInput { prompt, reply } => {
                self.agent_prompt = Some(prompt);
                self.agent_reply = Some(reply);
                self.agent_input.clear();
            }
        }
    }

    /// Marca todos los pasos del deploy como completados (la app quedó en vivo, posible-
    /// mente vía el agente). Evita mostrar un paso fallido + barra a medias en Live.
    pub fn complete_steps(&mut self) {
        for s in &mut self.steps {
            s.status = StepStatus::Done;
        }
    }

    /// Arranca el agente de arreglo sobre la VM del deploy fallido (tecla `a` en Error).
    pub fn start_fix_agent(&mut self) {
        self.agent_steps.clear();
        self.agent_outcome = None;
        self.agent_prompt = None;
        self.agent_input.clear();
        self.agent_reply = None;
        self.agent_cancel = Some(Arc::new(AtomicBool::new(false)));
        self.agent_busy = true;
        self.eyes = ("◉", "◉"); // ojos en trance: el agente está en el ruedo
        self.screen = Screen::Agent;
    }

    /// `Esc` durante el agente: aborta SIEMPRE, sin importar el estado. Prende la
    /// bandera (el loop la ve entre pasos), desbloquea un `need_secret` pendiente
    /// mandándole vacío, y suelta la UI de vuelta al panel de inmediato — el usuario
    /// nunca queda atrapado esperando a que el loop reaccione.
    pub fn cancel_agent(&mut self) {
        if let Some(flag) = &self.agent_cancel {
            flag.store(true, Ordering::Relaxed);
        }
        if let Some(reply) = self.agent_reply.take() {
            let _ = reply.send(String::new());
        }
        self.agent_busy = false;
        self.agent_prompt = None;
        self.agent_input.clear();
        self.agent_outcome = None;
        self.eyes = ("•", "•");
    }

    pub fn start_launch(&mut self) {
        // Limpia el estado de un deploy anterior — si no, la pantalla Launching
        // muestra el "tu app está en vivo" + URL del proyecto previo.
        self.url = None;
        self.live_at = None;
        self.sandbox_id = None;
        self.confirm_destroy = false;
        self.logs = None;
        self.running_idx = None;
        self.step_anchor = self.tick;
        self.steps = vec![
            Step::new("Creando VM persistente"),
            Step::new("Clonando + instalando + arrancando"),
            Step::new("Verificando que responda"),
            Step::new("Publicando puerto"),
        ];
        self.screen = Screen::Launching;
    }
}

/// Flujo OAuth (abre navegador). Envía AuthStatus → Authed / ValidateFailed.
pub fn spawn_oauth(tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let _ = tx.send(Msg::AuthStatus {
            text: "abriendo el navegador… autoriza en EasyBits".into(),
        });
        let creds = match oauth::run_oauth().await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Msg::ValidateFailed {
                    error: format!("OAuth: {e}"),
                });
                return;
            }
        };
        finish_with_token(creds.access_token, tx).await;
    });
}

/// Reconexión silenciosa con credenciales guardadas (refresca si vencieron).
pub fn spawn_reconnect(creds: Creds, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let _ = tx.send(Msg::AuthStatus {
            text: "reconectando…".into(),
        });
        let creds = if creds.is_expired() {
            match oauth::refresh(&creds).await {
                Ok(c) => {
                    oauth::save_creds(&c); // persiste el par rotado (si no, se desincroniza)
                    c
                }
                Err(e) => {
                    oauth::clear_creds(); // credenciales muertas → fuerza OAuth limpio
                    let _ = tx.send(Msg::ValidateFailed {
                        error: format!("sesión expirada ({e}) — conéctate de nuevo"),
                    });
                    return;
                }
            }
        } else {
            creds
        };
        finish_with_token(creds.access_token, tx).await;
    });
}

/// Construye el cliente con el access token y valida → Authed / ValidateFailed.
async fn finish_with_token(token: String, tx: UnboundedSender<Msg>) {
    let client = match Client::new(token) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Msg::ValidateFailed {
                error: e.to_string(),
            });
            return;
        }
    };
    let me = match client.validate().await {
        Ok(me) => me,
        Err(e) => {
            let _ = tx.send(Msg::ValidateFailed {
                error: e.to_string(),
            });
            return;
        }
    };
    let _ = tx.send(Msg::AuthStatus {
        text: "cargando tus apps…".into(),
    });
    match list_apps(&client).await {
        Ok(apps) => {
            let _ = tx.send(Msg::AppsLoaded {
                client,
                email: me.email,
                apps,
            });
        }
        Err(e) => {
            let _ = tx.send(Msg::AppsError { error: e });
        }
    }
}

/// URL pública de una VM expuesta, reconstruida del id+puerto (sin red).
/// Formato fijo de sandbox-host: `https://sb-<uuid>-<port>.sandboxes.easybits.cloud`.
pub fn public_url(id: &str, port: u16) -> String {
    format!(
        "https://{}-{}.sandboxes.easybits.cloud",
        id.replacen("sb_", "sb-", 1),
        port
    )
}

/// Lista las apps que publicó Ghosty Launch (VMs `gl:*` running) con su URL.
/// Reconstruye la URL (no llama expose por app) → reconexión con UNA sola request.
/// Lista las apps publicadas. `Err` = no se pudo leer (problema de red/API), que
/// es DISTINTO de "no hay apps" (`Ok(vec![])`). El caller no debe vaciar el panel
/// ante un `Err` — si no, un fallo transitorio parece "se borraron todas".
pub async fn list_apps(client: &Client) -> Result<Vec<AppEntry>, String> {
    let list = client.list_sandboxes().await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for s in list {
        let is_ours = s
            .name
            .as_deref()
            .map(|n| n.starts_with(APP_PREFIX))
            .unwrap_or(false);
        // Salta entradas sin id: no son borrables de forma segura (un id vacío
        // haría `DELETE /sandboxes/` = borrar todo).
        if is_ours && !s.sandbox_id.trim().is_empty() {
            let running = s.status == "running";
            out.push(AppEntry {
                name: display_name(&s.name),
                url: if running {
                    public_url(&s.sandbox_id, APP_PORT)
                } else {
                    String::new()
                },
                id: s.sandbox_id,
                running,
            });
        }
    }
    Ok(out)
}

/// Recarga el panel de apps (tras crear/destruir o volver del Live).
pub fn spawn_list_apps(client: Client, email: Option<String>, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        match list_apps(&client).await {
            Ok(apps) => {
                let _ = tx.send(Msg::AppsLoaded {
                    client,
                    email,
                    apps,
                });
            }
            Err(e) => {
                let _ = tx.send(Msg::AppsError { error: e });
            }
        }
    });
}

/// Destruye una VM y recarga el panel.
pub fn spawn_destroy_and_reload(
    client: Client,
    id: String,
    email: Option<String>,
    tx: UnboundedSender<Msg>,
) {
    tokio::spawn(async move {
        // Si el destroy falla, repórtalo (no sigas como si nada).
        if let Err(e) = client.destroy(&id).await {
            let _ = tx.send(Msg::Failed {
                error: format!("No se pudo borrar la app: {e}"),
            });
            return;
        }
        match list_apps(&client).await {
            Ok(apps) => {
                let _ = tx.send(Msg::AppsLoaded {
                    client,
                    email,
                    apps,
                });
            }
            // El borrado SÍ funcionó pero la recarga falló: no vacíes el panel.
            Err(e) => {
                let _ = tx.send(Msg::AppsError { error: e });
            }
        }
    });
}

/// Valida una llave/token (la pega el user) y carga el panel. Mismo camino que OAuth.
pub fn spawn_finish(token: String, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        finish_with_token(token, tx).await;
    });
}

/// Resuelve el logo a una URL pública: pasa URLs tal cual; sube rutas locales a
/// EasyBits (público). Devuelve "" si no hay logo o si falla (no rompe el launch).
async fn resolve_logo(client: &Client, raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return raw.to_string();
    }
    // Ruta local (drag&drop a veces la envuelve en comillas) → subir.
    let path = raw.trim_matches('\'').trim_matches('"').trim();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("logo");
    let ct = match name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };
    client
        .upload_public_file(name, ct, bytes)
        .await
        .unwrap_or_default()
}

/// Contrato del repo: receta de deploy declarada en `ghosty.toml`. Todo opcional;
/// lo que falte se auto-detecta (estilo Vercel: build si hay script, npm start).
#[derive(serde::Deserialize, Default)]
struct Manifest {
    /// "static" → CDN, "app" → VM. Si falta, se auto-detecta (package.json → app).
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    deploy: Deploy,
    #[serde(default)]
    resources: Resources,
}
#[derive(serde::Deserialize, Default)]
struct Deploy {
    install: Option<String>,
    build: Option<String>,
    start: Option<String>,
}
#[derive(serde::Deserialize, Default)]
struct Resources {
    /// Clase de VM: "s" | "m" | "l" | "xl". Override explícito gana sobre la
    /// auto-detección por peso del repo.
    size: Option<String>,
}

/// Baja `ghosty.toml` del repo (raw GitHub). Sin manifiesto → Default (auto-detect).
async fn fetch_manifest(ref_repo: &str) -> Manifest {
    let slug = repo_slug(ref_repo);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    for branch in ["main", "master"] {
        let url = format!("https://raw.githubusercontent.com/{slug}/{branch}/ghosty.toml");
        if let Ok(resp) = http.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(text) = resp.text().await {
                    if let Ok(m) = toml::from_str::<Manifest>(&text) {
                        return m;
                    }
                }
            }
        }
    }
    Manifest::default()
}

/// `owner/repo` desde cualquier forma: https, ssh (git@github.com:...), o slug.
fn repo_slug(repo: &str) -> String {
    let s = repo.trim();
    let s = s.strip_prefix("git@github.com:").unwrap_or(s);
    let s = s.strip_prefix("https://").unwrap_or(s);
    let s = s.strip_prefix("http://").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    s.trim_end_matches('/').trim_end_matches(".git").to_string()
}

/// URL de clone https público (normaliza SSH/slug). Repos privados no funcionan aún.
fn repo_https(repo: &str) -> String {
    format!("https://github.com/{}.git", repo_slug(repo))
}

/// Estático si `ghosty.toml type="static"`, o si el repo NO tiene `package.json`.
async fn detect_static(repo: &str) -> bool {
    let m = fetch_manifest(repo).await;
    if let Some(k) = m.kind {
        return k.eq_ignore_ascii_case("static");
    }
    let slug = repo_slug(repo);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    for branch in ["main", "master"] {
        let url = format!("https://raw.githubusercontent.com/{slug}/{branch}/package.json");
        if let Ok(r) = http.get(&url).send().await {
            if r.status().is_success() {
                return false; // tiene package.json → app
            }
        }
    }
    true // sin package.json → estático
}

/// Elige la clase de VM (s/m/l/xl) según el peso del repo. El override explícito
/// de `ghosty.toml [resources] size` gana. Heurística por package.json:
/// - sin build y ≤30 deps → s
/// - con build y ≤60 deps → m
/// - ≥60 deps o bundler pesado conocido → l
/// - next / monorepo (workspaces) / ≥120 deps → xl
async fn detect_size(repo: &str) -> &'static str {
    let m = fetch_manifest(repo).await;
    if let Some(s) = m.resources.size {
        return match s.to_ascii_lowercase().as_str() {
            "m" => "m",
            "l" => "l",
            "xl" => "xl",
            _ => "s",
        };
    }
    let slug = repo_slug(repo);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut pkg = String::new();
    for branch in ["main", "master"] {
        let url = format!("https://raw.githubusercontent.com/{slug}/{branch}/package.json");
        if let Ok(r) = http.get(&url).send().await {
            if r.status().is_success() {
                if let Ok(t) = r.text().await {
                    pkg = t;
                    break;
                }
            }
        }
    }
    if pkg.is_empty() {
        return "s";
    }
    let json: serde_json::Value = serde_json::from_str(&pkg).unwrap_or(serde_json::Value::Null);
    let count = |k: &str| {
        json.get(k)
            .and_then(|v| v.as_object())
            .map(|o| o.len())
            .unwrap_or(0)
    };
    let deps = count("dependencies") + count("devDependencies");
    let has_build = json.get("scripts").and_then(|s| s.get("build")).is_some();
    let has_workspaces = json.get("workspaces").is_some();
    let heavy = |name: &str| pkg.contains(&format!("\"{name}\""));
    let has_next = heavy("next");
    let heavy_bundler = heavy("vite")
        || heavy("@react-router/dev")
        || heavy("webpack")
        || heavy("@excalidraw/excalidraw");

    if has_next || has_workspaces || deps >= 120 {
        "xl"
    } else if deps >= 60 || heavy_bundler {
        "l"
    } else if has_build && deps <= 60 {
        "m"
    } else {
        "s"
    }
}

fn content_type_for(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn collect_files(
    dir: &std::path::Path,
    base: &std::path::Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name() == *".git" {
                continue;
            }
            if p.is_dir() {
                collect_files(&p, base, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push((rel.to_string_lossy().replace('\\', "/"), p));
            }
        }
    }
}

/// Crea: detecta tipo y enruta. Estático → publica al CDN; app → va a Consent (VM).
pub fn spawn_create(client: Client, repo: String, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        if !detect_static(&repo).await {
            let _ = tx.send(Msg::AppCreate { repo });
            return;
        }
        let _ = tx.send(Msg::StaticStart);
        match publish_static(&client, &repo, &tx).await {
            Ok(url) => {
                let _ = tx.send(Msg::Live { url });
            }
            Err(e) => {
                let _ = tx.send(Msg::Step {
                    idx: 2,
                    status: StepStatus::Failed,
                    detail: e.to_string(),
                });
                let _ = tx.send(Msg::Failed {
                    error: e.to_string(),
                });
            }
        }
    });
}

/// Publica un sitio estático al CDN de EasyBits: clone local → crear website → subir.
async fn publish_static(
    client: &Client,
    repo: &str,
    tx: &UnboundedSender<Msg>,
) -> anyhow::Result<String> {
    let step = |idx, status, detail: &str| {
        let _ = tx.send(Msg::Step {
            idx,
            status,
            detail: detail.to_string(),
        });
    };

    step(0, StepStatus::Running, "");
    let dir = std::env::temp_dir().join("ghosty-launch-static");
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let ok = tokio::process::Command::new("git")
        .args([
            "clone",
            "--quiet",
            "--depth",
            "1",
            &repo_https(repo),
            &dir.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("git clone falló (¿el repo es público?)");
    }
    step(0, StepStatus::Done, "repo clonado");

    step(1, StepStatus::Running, "");
    let name = repo_slug(repo)
        .rsplit('/')
        .next()
        .unwrap_or("sitio")
        .to_string();
    let (id, url) = client.create_website(&name).await?;
    step(1, StepStatus::Done, "sitio creado");

    step(2, StepStatus::Running, "");
    let mut files = Vec::new();
    collect_files(&dir, &dir, &mut files);
    if files.is_empty() {
        anyhow::bail!("el repo no tiene archivos");
    }
    for (rel, abs) in &files {
        let bytes = tokio::fs::read(abs).await.unwrap_or_default();
        client
            .upload_website_file(&id, rel, content_type_for(rel), bytes)
            .await?;
    }
    step(
        2,
        StepStatus::Done,
        format!("{} archivos al CDN", files.len()).as_str(),
    );
    let _ = tokio::fs::remove_dir_all(&dir).await;
    Ok(url)
}

/// Corre el pipeline completo en background, reportando cada paso.
/// `app_name`/`accent`/`logo` se inyectan como env a la app (personalización real).
pub fn spawn_launch(
    client: Client,
    tx: UnboundedSender<Msg>,
    repo: String,
    app_name: String,
    accent: String,
    logo: String,
    envs: Vec<(String, String)>,
) {
    tokio::spawn(async move {
        let ref_repo = repo_https(&repo); // normaliza SSH/slug → https público
        let app_name = safe_name(&app_name);
        let logo_url = resolve_logo(&client, &logo).await;
        let manifest = fetch_manifest(&ref_repo).await;
        // Clase de VM según el peso del repo (s/m/l/xl). m+ trae un disco /app.
        let size = detect_size(&ref_repo).await;

        // Paso 0 — crear VM persistente + poll hasta running.
        let _ = tx.send(Msg::Step {
            idx: 0,
            status: StepStatus::Running,
            detail: format!("VM tamaño {size}"),
        });
        let sandbox = match client
            .create_sandbox(TEMPLATE, true, &marker_name(&app_name), size)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Msg::Step {
                    idx: 0,
                    status: StepStatus::Failed,
                    detail: e.to_string(),
                });
                let _ = tx.send(Msg::Failed {
                    error: e.to_string(),
                });
                return;
            }
        };
        let id = sandbox.sandbox_id.clone();
        let _ = tx.send(Msg::SandboxCreated { id: id.clone() });

        // Poll status (hasta ~60s).
        let mut running = sandbox.status == "running";
        for _ in 0..30 {
            if running {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            match client.get_sandbox(&id).await {
                Ok(s) => {
                    running = s.status == "running";
                    let _ = tx.send(Msg::Step {
                        idx: 0,
                        status: StepStatus::Running,
                        detail: format!("estado: {}", s.status),
                    });
                }
                Err(e) => {
                    let _ = tx.send(Msg::Step {
                        idx: 0,
                        status: StepStatus::Failed,
                        detail: e.to_string(),
                    });
                    let _ = tx.send(Msg::Failed {
                        error: e.to_string(),
                    });
                    return;
                }
            }
        }
        if !running {
            let err = "La VM no llegó a 'running' a tiempo".to_string();
            let _ = tx.send(Msg::Step {
                idx: 0,
                status: StepStatus::Failed,
                detail: err.clone(),
            });
            let _ = tx.send(Msg::Failed { error: err });
            return;
        }
        let _ = tx.send(Msg::Step {
            idx: 0,
            status: StepStatus::Done,
            detail: format!("VM {id} lista"),
        });

        // Paso 1 — clone + install + start (un solo exec, arranque en background).
        let _ = tx.send(Msg::Step {
            idx: 1,
            status: StepStatus::Running,
            detail: String::new(),
        });
        // Receta del contrato (ghosty.toml) o auto-detect.
        // Auto-install: SIN --omit=dev — el build (vite/RRv7) necesita devDeps como
        // @react-router/dev. Tras el build podamos devDeps para liberar disco.
        // Las envs que dio el usuario al deploy se PERSISTEN en el override durable, para
        // que el agente las vea como ya configuradas (y no las re-pida si el deploy falla).
        crate::recipe::persist_envs(&app_name, &envs);
        // Override local del agente (arreglo durable) gana sobre el ghosty.toml del repo.
        let ovr = crate::recipe::load(&app_name);
        let install_recipe = ovr.install.clone().or_else(|| manifest.deploy.install.clone());
        let build_recipe = ovr.build.clone().or_else(|| manifest.deploy.build.clone());
        let start_recipe = ovr.start.clone().or_else(|| manifest.deploy.start.clone());

        let auto_install = install_recipe.is_none();
        let install = install_recipe.unwrap_or_else(|| {
            "if [ -f package-lock.json ]; then npm ci || npm install; else npm install; fi".into()
        });
        let build_step = match build_recipe {
            Some(b) => format!("{b}; "),
            // Auto-detect: corre el build solo si package.json declara script `build`.
            None => "if node -e \"process.exit(require('./package.json').scripts&&require('./package.json').scripts.build?0:1)\" 2>/dev/null; then npm run build; fi; ".into(),
        };
        // En auto-install podamos devDeps tras el build (igual que el Dockerfile).
        let prune = if auto_install {
            "npm prune --omit=dev 2>/dev/null || true; "
        } else {
            ""
        };
        let start = start_recipe.unwrap_or_else(|| "npm start".into());
        // Envs del override (arreglo durable del agente) fusionados sobre los del usuario.
        let envs = crate::recipe::merge_envs(envs, &ovr.envs);
        // Con tamaño m+ EasyBits adjunta un volumen ext4 en /app (no vacío:
        // lost+found) → clonar en /app/src para que `git clone` no falle. Con "s"
        // no hay volumen, /app es directorio normal del rootfs.
        let workdir = if size == "s" { "/app" } else { "/app/src" };
        // Heap de Node escalado a la RAM de la VM: por default Node topa el
        // old-space en ~2GB y los builds de vite/RRv7 hacen OOM aunque la VM
        // tenga más RAM. ~75% de la RAM de la clase.
        let node_heap = match size {
            "xl" => 6144,
            "l" => 3072,
            "m" => 1536,
            _ => 0,
        };
        let node_env = if node_heap > 0 {
            format!("export NODE_OPTIONS=--max-old-space-size={node_heap}; ")
        } else {
            String::new()
        };
        // Envs del usuario (de .env o tecleadas) — van ANTES de las APP_*/PORT para
        // que nuestra personalización y el puerto siempre ganen. Cada valor va
        // quoteado a prueba de shell.
        let user_env: String = envs
            .iter()
            .filter(|(k, v)| is_env_key(k) && !v.is_empty())
            .map(|(k, v)| format!("{k}={} ", sh_squote(v)))
            .collect();
        let cmd = format!(
            "set -e; {node_env}rm -rf {workdir}; mkdir -p {workdir}; git clone --depth 1 {ref_repo} {workdir}; cd {workdir}; {install}; {build_step}{prune}({user_env}APP_NAME='{app_name}' APP_ACCENT='{accent}' APP_LOGO='{logo_url}' PORT={APP_PORT} setsid nohup {start} > /tmp/app.log 2>&1 &); sleep 2; echo GHOSTY_DEPLOY_DONE"
        );
        // Deploy en background + polling (resiliente: la VM saturada por el build no
        // tira la conexión, cada poll es una llamada corta y reintentable).
        let exec_id = match client.exec_background(&id, &cmd).await {
            Ok(b) => b.exec_id,
            Err(e) => {
                let _ = tx.send(Msg::Step {
                    idx: 1,
                    status: StepStatus::Failed,
                    detail: e.to_string(),
                });
                let _ = tx.send(Msg::Failed {
                    error: e.to_string(),
                });
                return;
            }
        };
        const MAX_POLLS: u32 = 240; // 240 × 3s = 12 min
        const MAX_CONSEC_ERRORS: u32 = 30; // ~90s seguidos sin respuesta → rendirse
        let mut consec_errors = 0u32;
        let mut finished = false;
        for _ in 0..MAX_POLLS {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            match client.exec_status(&id, &exec_id).await {
                Ok(st) if st.status == "exited" => {
                    let code = st.exit_code.unwrap_or(-1);
                    if code == 0 {
                        let _ = tx.send(Msg::Step {
                            idx: 1,
                            status: StepStatus::Done,
                            detail: "app arrancada".to_string(),
                        });
                    } else {
                        let detail = trim_log(&format!("{}{}", st.stdout, st.stderr));
                        let _ = tx.send(Msg::Step {
                            idx: 1,
                            status: StepStatus::Failed,
                            detail: detail.clone(),
                        });
                        let _ = tx.send(Msg::Failed {
                            error: format!("deploy exit {code}: {detail}"),
                        });
                        return;
                    }
                    finished = true;
                    break;
                }
                Ok(_) => {
                    consec_errors = 0;
                    let _ = tx.send(Msg::Step {
                        idx: 1,
                        status: StepStatus::Running,
                        detail: "instalando y compilando…".to_string(),
                    });
                }
                Err(_) => {
                    // 5xx transitorio: el agente in-VM está ocupado con el build. Reintentar.
                    consec_errors += 1;
                    if consec_errors >= MAX_CONSEC_ERRORS {
                        let err = "El deploy dejó de responder (VM saturada demasiado tiempo)"
                            .to_string();
                        let _ = tx.send(Msg::Step {
                            idx: 1,
                            status: StepStatus::Failed,
                            detail: err.clone(),
                        });
                        let _ = tx.send(Msg::Failed { error: err });
                        return;
                    }
                }
            }
        }
        if !finished {
            let err = "El deploy no terminó a tiempo (12 min)".to_string();
            let _ = tx.send(Msg::Step {
                idx: 1,
                status: StepStatus::Failed,
                detail: err.clone(),
            });
            let _ = tx.send(Msg::Failed { error: err });
            return;
        }

        // Paso 2 — health check: ¿la app realmente responde? El proceso se lanzó en
        // background; pudo crashear al arrancar (env faltante, error de runtime) y la
        // URL quedaría muerta. Probamos http://127.0.0.1:PORT DESDE la VM, con
        // reintentos mientras bootea. Si no responde, traemos /tmp/app.log.
        let _ = tx.send(Msg::Step {
            idx: 2,
            status: StepStatus::Running,
            detail: format!("probando http://127.0.0.1:{APP_PORT}…"),
        });
        let healthy = health_check(&client, &id).await;
        if !healthy {
            let logs = fetch_app_log(&client, &id).await;
            let tail = trim_log(&logs);
            let _ = tx.send(Msg::Step {
                idx: 2,
                status: StepStatus::Failed,
                detail: "la app no respondió en el puerto".to_string(),
            });
            // Guarda el log completo (para el visor) y falla con el final del log.
            let _ = tx.send(Msg::Logs { text: logs });
            let _ = tx.send(Msg::Failed {
                error: if tail.is_empty() {
                    "la app no respondió en el puerto (sin logs)".to_string()
                } else {
                    format!("la app no respondió. Últimas líneas del log:\n{tail}")
                },
            });
            return;
        }
        let _ = tx.send(Msg::Step {
            idx: 2,
            status: StepStatus::Done,
            detail: "responde 🟢".to_string(),
        });

        // Paso 3 — exponer puerto.
        let _ = tx.send(Msg::Step {
            idx: 3,
            status: StepStatus::Running,
            detail: String::new(),
        });
        match client.expose(&id, APP_PORT).await {
            Ok(exp) => {
                // No cantamos victoria con el loopback: probamos la URL pública de verdad.
                let _ = tx.send(Msg::Step {
                    idx: 3,
                    status: StepStatus::Running,
                    detail: "verificando la URL pública…".to_string(),
                });
                finalize_live(exp.url, &tx).await;
            }
            Err(e) => {
                let _ = tx.send(Msg::Step {
                    idx: 3,
                    status: StepStatus::Failed,
                    detail: e.to_string(),
                });
                let _ = tx.send(Msg::Failed {
                    error: e.to_string(),
                });
            }
        }
    });
}

/// Corre un comando one-shot en la VM y devuelve su BgStatus final (o None si no
/// terminó / falló la conexión). Polla cada 2s hasta `max_polls`.
pub(crate) async fn exec_oneshot(
    client: &Client,
    id: &str,
    cmd: &str,
    max_polls: u32,
) -> Option<crate::easybits::BgStatus> {
    let exec_id = client.exec_background(id, cmd).await.ok()?.exec_id;
    for _ in 0..max_polls {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if let Ok(st) = client.exec_status(id, &exec_id).await {
            if st.status == "exited" {
                return Some(st);
            }
        }
    }
    None
}

/// ¿La app responde en 127.0.0.1:PORT dentro de la VM? Reintenta ~40s mientras
/// bootea. Usa Node (garantizado en el template) — sin depender de curl. <500 = sano.
pub(crate) async fn health_check(client: &Client, id: &str) -> bool {
    let probe = format!(
        "require('http').get({{host:'127.0.0.1',port:{APP_PORT},timeout:2000}},r=>process.exit(r.statusCode<500?0:1)).on('error',()=>process.exit(1)).on('timeout',function(){{this.destroy();process.exit(1)}})"
    );
    // Bucle POSIX (sin `seq`): 20 intentos × ~2s.
    let cmd = format!(
        "i=0; while [ $i -lt 20 ]; do node -e \"{probe}\" && exit 0; i=$((i+1)); sleep 2; done; exit 1"
    );
    // El bucle puede tardar ~40s → damos margen de polling (30 × 2s = 60s).
    matches!(exec_oneshot(client, id, &cmd, 30).await, Some(st) if st.exit_code == Some(0))
}

/// ¿La URL PÚBLICA responde de verdad? Esto valida lo que ve el navegador del usuario
/// (handshake TLS del edge + HTTP <500), NO el loopback interno. Sin esto, launch canta
/// "🟢 en vivo" aunque el proxy/TLS de EasyBits no sirva la URL. Reintenta para dar
/// margen a la propagación del proxy/certificado.
pub(crate) async fn public_ok(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
    else {
        return false;
    };
    for attempt in 0..5 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().as_u16() < 500 {
                return true;
            }
        }
    }
    false
}

/// Cierra el deploy con honestidad: prueba la URL pública y manda `Live` solo si responde
/// de verdad; si no, `LiveUnverified` (corre, pero la URL no sirve aún). Lo usan las tres
/// rutas que dejan una app en vivo (deploy, reconfigure, agente).
pub(crate) async fn finalize_live(url: String, tx: &UnboundedSender<Msg>) {
    if public_ok(&url).await {
        let _ = tx.send(Msg::Live { url });
    } else {
        let _ = tx.send(Msg::LiveUnverified { url });
    }
}

/// Trae las últimas líneas de `/tmp/app.log` de la VM (stdout/stderr de la app).
pub(crate) async fn fetch_app_log(client: &Client, id: &str) -> String {
    match exec_oneshot(client, id, "tail -n 200 /tmp/app.log 2>/dev/null", 15).await {
        Some(st) => {
            let out = format!("{}{}", st.stdout, st.stderr);
            if out.trim().is_empty() {
                "(log vacío — la app no escribió nada en /tmp/app.log)".to_string()
            } else {
                out
            }
        }
        None => "(no se pudieron leer los logs de la VM)".to_string(),
    }
}

/// Trae los logs de la VM a la vista de Logs (botón `l`).
pub fn spawn_fetch_logs(client: Client, id: String, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let logs = fetch_app_log(&client, &id).await;
        let _ = tx.send(Msg::Logs { text: logs });
    });
}

/// Reinicia una app EN SU VM con nuevas envs, sin reclonar (el código ya está en
/// /app o /app/src). Detiene el proceso viejo (el que tiene PORT en su environ),
/// re-arranca con las envs nuevas y verifica que responda. Reusa la URL ya expuesta.
pub fn spawn_reconfigure(
    client: Client,
    tx: UnboundedSender<Msg>,
    id: String,
    app_name: String,
    envs: Vec<(String, String)>,
) {
    tokio::spawn(async move {
        // Persistimos las envs en el override durable (igual que el deploy): el agente
        // las verá como ya configuradas y no las re-pedirá.
        crate::recipe::persist_envs(&app_name, &envs);
        // Paso 0 — detener el proceso viejo + re-arrancar con las nuevas envs.
        let _ = tx.send(Msg::Step {
            idx: 0,
            status: StepStatus::Running,
            detail: String::new(),
        });
        let user_env: String = envs
            .iter()
            .filter(|(k, v)| is_env_key(k) && !v.is_empty())
            .map(|(k, v)| format!("{k}={} ", sh_squote(v)))
            .collect();
        let safe = safe_name(&app_name);
        // Mata SOLO los procesos cuyo environ tenga PORT=<APP_PORT> (= nuestra app),
        // nunca el agente in-VM. Pura shell + /proc, sin depender de ss/lsof.
        let killer = format!(
            "for pid in $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do if grep -qa 'PORT={APP_PORT}' /proc/$pid/environ 2>/dev/null; then [ \"$pid\" != \"$$\" ] && kill $pid 2>/dev/null; fi; done"
        );
        // Detecta el workdir (donde quedó el package.json) y el start (ghosty.toml o
        // npm start). Sin set -e: los errores se ven en el health check.
        let cmd = format!(
            "for d in /app/src /app; do [ -f \"$d/package.json\" ] && WD=\"$d\" && break; done; [ -z \"$WD\" ] && {{ echo NO_APP; exit 1; }}; cd \"$WD\"; START=$(sed -n 's/^[[:space:]]*start[[:space:]]*=[[:space:]]*\"\\(.*\\)\".*/\\1/p' \"$WD/ghosty.toml\" 2>/dev/null | head -1); [ -z \"$START\" ] && START=\"npm start\"; {killer}; sleep 1; rm -f /tmp/app.log; ({user_env}APP_NAME='{safe}' PORT={APP_PORT} setsid nohup $START > /tmp/app.log 2>&1 &); sleep 2; echo GHOSTY_RECONFIG_DONE"
        );
        match exec_oneshot(&client, &id, &cmd, 60).await {
            Some(st) if st.exit_code == Some(0) => {
                let _ = tx.send(Msg::Step {
                    idx: 0,
                    status: StepStatus::Done,
                    detail: String::new(),
                });
            }
            Some(st) => {
                let detail = if st.stdout.contains("NO_APP") {
                    "no encontré el código de la app en la VM (¿se borró /app?)".to_string()
                } else {
                    trim_log(&format!("{}{}", st.stdout, st.stderr))
                };
                let _ = tx.send(Msg::Step {
                    idx: 0,
                    status: StepStatus::Failed,
                    detail: detail.clone(),
                });
                let _ = tx.send(Msg::Failed {
                    error: format!("No se pudo reiniciar: {detail}"),
                });
                return;
            }
            None => {
                let err = "El reinicio no respondió (VM ocupada o caída)".to_string();
                let _ = tx.send(Msg::Step {
                    idx: 0,
                    status: StepStatus::Failed,
                    detail: err.clone(),
                });
                let _ = tx.send(Msg::Failed { error: err });
                return;
            }
        }

        // Paso 1 — confirmar que arrancó (es el mismo exec; lo marcamos hecho).
        let _ = tx.send(Msg::Step {
            idx: 1,
            status: StepStatus::Done,
            detail: "proceso re-lanzado".to_string(),
        });

        // Paso 2 — health check (igual que el deploy).
        let _ = tx.send(Msg::Step {
            idx: 2,
            status: StepStatus::Running,
            detail: format!("probando http://127.0.0.1:{APP_PORT}…"),
        });
        if !health_check(&client, &id).await {
            let logs = fetch_app_log(&client, &id).await;
            let tail = trim_log(&logs);
            let _ = tx.send(Msg::Step {
                idx: 2,
                status: StepStatus::Failed,
                detail: "la app no respondió tras reiniciar".to_string(),
            });
            let _ = tx.send(Msg::Logs { text: logs });
            let _ = tx.send(Msg::Failed {
                error: if tail.is_empty() {
                    "la app no respondió tras reiniciar (sin logs)".to_string()
                } else {
                    format!("sigue sin responder. Últimas líneas del log:\n{tail}")
                },
            });
            return;
        }
        let _ = tx.send(Msg::Step {
            idx: 2,
            status: StepStatus::Done,
            detail: "responde 🟢".to_string(),
        });
        // La VM ya tenía el puerto expuesto → reusamos la misma URL pública, pero la
        // verificamos de verdad antes de cantar verde.
        finalize_live(public_url(&id, APP_PORT), &tx).await;
    });
}

pub(crate) fn trim_log(s: &str) -> String {
    let s = s.trim();
    if s.len() > 200 {
        format!("…{}", &s[s.len() - 200..])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_parse_basico() {
        let env = parse_dotenv(
            "# comentario\nexport FOO=bar\nBAZ=\"con espacios\"\nQUX='simple'\n\nMALA LINEA\n=sin_clave\n",
        );
        assert_eq!(
            env,
            vec![
                ("FOO".into(), "bar".into()),
                ("BAZ".into(), "con espacios".into()),
                ("QUX".into(), "simple".into()),
            ]
        );
    }

    #[test]
    fn dotenv_ultima_definicion_gana() {
        let env = parse_dotenv("A=1\nA=2\n");
        assert_eq!(env, vec![("A".into(), "2".into())]);
    }

    #[test]
    fn claves_validas() {
        assert!(is_env_key("FOO_BAR"));
        assert!(is_env_key("_x9"));
        assert!(!is_env_key("9foo"));
        assert!(!is_env_key("foo-bar"));
        assert!(!is_env_key(""));
    }

    #[test]
    fn squote_a_prueba_de_inyeccion() {
        // Un valor malicioso queda contenido dentro de comillas simples.
        assert_eq!(sh_squote("a'; rm -rf /; '"), "'a'\\''; rm -rf /; '\\'''");
        assert_eq!(sh_squote("simple"), "'simple'");
    }

    #[test]
    fn upsert_agrega_actualiza_y_quita() {
        let mut app = App::new();
        assert!(app.upsert_env("FOO=bar"));
        assert_eq!(app.envs, vec![("FOO".into(), "bar".into())]);
        // Actualiza el valor existente, no duplica.
        app.upsert_env("FOO=baz");
        assert_eq!(app.envs, vec![("FOO".into(), "baz".into())]);
        // Valor vacío elimina la clave.
        app.upsert_env("FOO=");
        assert!(app.envs.is_empty());
        // Clave inválida se ignora.
        assert!(!app.upsert_env("1bad=x"));
        assert!(!app.upsert_env("sin_igual"));
    }
}
