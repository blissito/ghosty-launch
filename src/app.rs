//! Estado del TUI (máquina de estados determinista) + orquestación del launch.

use crate::easybits::Client;
use crate::oauth::{self, Creds};
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
    Launching,
    Live,
    Error,
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
    Failed {
        error: String,
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
    /// URL del repo a publicar (pantalla Create).
    pub repo_input: String,
    pub error: Option<String>,
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
            repo_input: String::new(),
            error: None,
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

    /// Empezar a crear: limpia la personalización y pide el repo (prefilla el default).
    pub fn start_create(&mut self) {
        self.key_input.clear();
        self.logo_input.clear();
        self.accent_idx = 0;
        self.custom_hex = "#".into();
        self.focus = 0;
        self.error = None;
        self.repo_input = App::ref_repo();
        self.screen = Screen::Create;
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
                self.screen = Screen::Live;
            }
            Msg::Failed { error } => {
                self.error = Some(error);
                self.screen = Screen::Error;
            }
        }
    }

    pub fn start_launch(&mut self) {
        self.steps = vec![
            Step::new("Creando VM persistente"),
            Step::new("Clonando + instalando + arrancando"),
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
                Ok(c) => c,
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
    let apps = list_apps(&client).await;
    let _ = tx.send(Msg::AppsLoaded {
        client,
        email: me.email,
        apps,
    });
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
pub async fn list_apps(client: &Client) -> Vec<AppEntry> {
    let list = match client.list_sandboxes().await {
        Ok(l) => l,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for s in list {
        let is_ours = s
            .name
            .as_deref()
            .map(|n| n.starts_with(APP_PREFIX))
            .unwrap_or(false);
        if is_ours && s.status == "running" {
            out.push(AppEntry {
                name: display_name(&s.name),
                url: public_url(&s.sandbox_id, APP_PORT),
                id: s.sandbox_id,
            });
        }
    }
    out
}

/// Recarga el panel de apps (tras crear/destruir o volver del Live).
pub fn spawn_list_apps(client: Client, email: Option<String>, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let apps = list_apps(&client).await;
        let _ = tx.send(Msg::AppsLoaded {
            client,
            email,
            apps,
        });
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
        let _ = client.destroy(&id).await;
        let apps = list_apps(&client).await;
        let _ = tx.send(Msg::AppsLoaded {
            client,
            email,
            apps,
        });
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
}
#[derive(serde::Deserialize, Default)]
struct Deploy {
    install: Option<String>,
    build: Option<String>,
    start: Option<String>,
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
) {
    tokio::spawn(async move {
        let ref_repo = repo_https(&repo); // normaliza SSH/slug → https público
        let app_name = safe_name(&app_name);
        let logo_url = resolve_logo(&client, &logo).await;
        let manifest = fetch_manifest(&ref_repo).await;

        // Paso 0 — crear VM persistente + poll hasta running.
        let _ = tx.send(Msg::Step {
            idx: 0,
            status: StepStatus::Running,
            detail: String::new(),
        });
        let sandbox = match client
            .create_sandbox(TEMPLATE, true, &marker_name(&app_name))
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
        let install = manifest.deploy.install.clone().unwrap_or_else(|| {
            "if [ -f package-lock.json ]; then npm ci --omit=dev || npm install --omit=dev; else npm install --omit=dev; fi".into()
        });
        let build_step = match manifest.deploy.build.clone() {
            Some(b) => format!("{b}; "),
            // Auto-detect: corre el build solo si package.json declara script `build`.
            None => "if node -e \"process.exit(require('./package.json').scripts&&require('./package.json').scripts.build?0:1)\" 2>/dev/null; then npm run build; fi; ".into(),
        };
        let start = manifest
            .deploy
            .start
            .clone()
            .unwrap_or_else(|| "npm start".into());
        let cmd = format!(
            "set -e; rm -rf /app; git clone --depth 1 {ref_repo} /app; cd /app; {install}; {build_step}(APP_NAME='{app_name}' APP_ACCENT='{accent}' APP_LOGO='{logo_url}' PORT={APP_PORT} nohup {start} > /tmp/app.log 2>&1 &); sleep 3; echo started"
        );
        match client.exec(&id, &cmd, 300).await {
            Ok(r) if r.exit_code == 0 => {
                let _ = tx.send(Msg::Step {
                    idx: 1,
                    status: StepStatus::Done,
                    detail: "app arrancada".to_string(),
                });
            }
            Ok(r) => {
                let detail = trim_log(&format!("{}{}", r.stdout, r.stderr));
                let _ = tx.send(Msg::Step {
                    idx: 1,
                    status: StepStatus::Failed,
                    detail: detail.clone(),
                });
                let _ = tx.send(Msg::Failed {
                    error: format!("deploy exit {}: {detail}", r.exit_code),
                });
                return;
            }
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
        }

        // Paso 2 — exponer puerto.
        let _ = tx.send(Msg::Step {
            idx: 2,
            status: StepStatus::Running,
            detail: String::new(),
        });
        match client.expose(&id, APP_PORT).await {
            Ok(exp) => {
                let _ = tx.send(Msg::Step {
                    idx: 2,
                    status: StepStatus::Done,
                    detail: exp.url.clone(),
                });
                let _ = tx.send(Msg::Live { url: exp.url });
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

fn trim_log(s: &str) -> String {
    let s = s.trim();
    if s.len() > 200 {
        format!("…{}", &s[s.len() - 200..])
    } else {
        s.to_string()
    }
}
