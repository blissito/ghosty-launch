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
/// Nombre con el que marcamos la VM para reencontrarla en runs posteriores.
pub const APP_NAME: &str = "ghosty-launch";

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
    Consent,
    Customize,
    Launching,
    Live,
    Error,
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
    Validated {
        email: Option<String>,
    },
    ValidateFailed {
        error: String,
    },
    AuthStatus {
        text: String,
    },
    Authed {
        client: Client,
        email: Option<String>,
    },
    /// Ya había una app publicada: saltamos directo al Live.
    Resumed {
        client: Client,
        email: Option<String>,
        id: String,
        url: String,
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
    /// Personalización elegida en la pantalla Customize.
    pub app_name: String,
    pub accent_idx: usize,
    /// Foco en Customize: 0 = nombre, 1 = color, 2 = logo.
    pub focus: u8,
    /// Hex tecleado cuando el acento es "custom".
    pub custom_hex: String,
    /// Ruta local o URL del logo (drag&drop pega la ruta).
    pub logo_input: String,
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
            app_name: String::new(),
            accent_idx: 0,
            focus: 0,
            custom_hex: "#".to_string(),
            logo_input: String::new(),
            error: None,
            should_quit: false,
        }
    }

    pub fn ref_repo() -> String {
        std::env::var("GHOSTY_REF_REPO").unwrap_or_else(|_| DEFAULT_REF_REPO.to_string())
    }

    /// Aplica un mensaje de tarea async al estado.
    pub fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Validated { email } => {
                self.validating = false;
                self.email = email;
                self.screen = Screen::Consent;
            }
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
            Msg::Authed { client, email } => {
                self.auth_busy = false;
                self.validating = false;
                self.client = Some(client);
                self.email = email;
                self.screen = Screen::Consent;
            }
            Msg::Resumed {
                client,
                email,
                id,
                url,
            } => {
                self.auth_busy = false;
                self.validating = false;
                self.client = Some(client);
                self.email = email;
                self.sandbox_id = Some(id);
                self.url = Some(url);
                self.screen = Screen::Live;
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
    // ¿Ya hay una app publicada por nosotros? Si sí, saltamos al Live.
    let _ = tx.send(Msg::AuthStatus {
        text: "buscando tu app publicada…".into(),
    });
    if let Some((id, url)) = find_live_app(&client).await {
        let _ = tx.send(Msg::Resumed {
            client,
            email: me.email,
            id,
            url,
        });
    } else {
        let _ = tx.send(Msg::Authed {
            client,
            email: me.email,
        });
    }
}

/// Busca una VM nuestra (`APP_NAME`) en estado running y devuelve (id, url pública).
async fn find_live_app(client: &Client) -> Option<(String, String)> {
    let list = client.list_sandboxes().await.ok()?;
    for s in list {
        if s.name.as_deref() == Some(APP_NAME) && s.status == "running" {
            if let Ok(exp) = client.expose(&s.sandbox_id, APP_PORT).await {
                return Some((s.sandbox_id, exp.url));
            }
        }
    }
    None
}

/// Valida la llave en background. Envía Validated / ValidateFailed.
pub fn spawn_validate(client: Client, tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        match client.validate().await {
            Ok(me) => {
                let _ = tx.send(Msg::Validated { email: me.email });
            }
            Err(e) => {
                let _ = tx.send(Msg::ValidateFailed {
                    error: e.to_string(),
                });
            }
        }
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

/// Corre el pipeline completo en background, reportando cada paso.
/// `app_name`/`accent`/`logo` se inyectan como env a la app (personalización real).
pub fn spawn_launch(
    client: Client,
    tx: UnboundedSender<Msg>,
    app_name: String,
    accent: String,
    logo: String,
) {
    tokio::spawn(async move {
        let ref_repo = App::ref_repo();
        let app_name = safe_name(&app_name);
        let logo_url = resolve_logo(&client, &logo).await;

        // Paso 0 — crear VM persistente + poll hasta running.
        let _ = tx.send(Msg::Step {
            idx: 0,
            status: StepStatus::Running,
            detail: String::new(),
        });
        let sandbox = match client.create_sandbox(TEMPLATE, true, APP_NAME).await {
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
        let cmd = format!(
            "set -e; rm -rf /app; git clone --depth 1 {ref_repo} /app; cd /app; if [ -f package-lock.json ]; then npm ci --omit=dev; else npm install --omit=dev; fi; (APP_NAME='{app_name}' APP_ACCENT='{accent}' APP_LOGO='{logo_url}' PORT={APP_PORT} nohup npm start > /tmp/app.log 2>&1 &); sleep 3; echo started"
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
