//! Ghosty Launch — TUI que clona un repo y lo publica live en una VM de EasyBits.
//! MVP: camino feliz determinista (llave → consentimiento → deploy → URL live).

mod app;
mod debug;
mod easybits;
mod oauth;
mod ui;

use anyhow::Result;
use app::{
    spawn_destroy_and_reload, spawn_finish, spawn_launch, spawn_list_apps, spawn_oauth,
    spawn_reconnect, App, Screen,
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    // Modo headless de debug: corre el pipeline imprimiendo crudo, sin TUI.
    if std::env::args().any(|a| a == "--debug") {
        return debug::run().await;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    if let Err(e) = res {
        eprintln!("ghosty-launch: {e:#}");
    }
    Ok(())
}

async fn run<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut app = App::new();
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Limpia la pantalla alterna: sin esto, el contenido previo del terminal se
    // cuela alrededor de la tarjeta (ratatui solo pinta las celdas que cambian).
    terminal.clear()?;

    // Reconexión silenciosa si hay credenciales guardadas (refresca si vencieron).
    if let Some(creds) = oauth::load_creds() {
        app.auth_busy = true;
        app.auth_status = "reconectando…".into();
        spawn_reconnect(creds, tx.clone());
    }

    loop {
        // Drena mensajes de tareas async.
        while let Ok(msg) = rx.try_recv() {
            app.apply(msg);
        }

        app.tick = app.tick.wrapping_add(1);
        terminal.draw(|f| ui::render(f, &app))?;

        if app.should_quit {
            break;
        }

        // Input no-bloqueante (50ms): deja respirar a las tareas async.
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    let ctrl_c = key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL);
                    if ctrl_c {
                        app.should_quit = true;
                        continue;
                    }
                    handle_key(&mut app, key.code, &tx);
                }
                // Pegado de la llave: el terminal lo manda como un evento Paste
                // (no como teclas), evitando el aviso de "bracketed paste".
                Event::Paste(text) if app.screen == Screen::KeyEntry && app.paste_mode => {
                    for c in text.chars() {
                        if !c.is_whitespace() {
                            app.key_input.push(c);
                        }
                    }
                    // Pegar la llave valida solo — sin Enter.
                    submit_key(&mut app, &tx);
                }
                // En Customize, pegar (o arrastrar) inserta en el campo enfocado:
                // ruta del logo, o nombre. Útil para drag&drop del archivo.
                Event::Paste(text) if app.screen == Screen::Customize => {
                    let clean: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                    if app.focus == app::FOCUS_LOGO {
                        app.logo_input.push_str(&clean);
                    } else if app.focus == app::FOCUS_NAME {
                        app.key_input.push_str(clean.trim());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Valida la llave/token pegado y carga el panel (mismo camino que OAuth).
fn submit_key(app: &mut App, tx: &mpsc::UnboundedSender<app::Msg>) {
    let key = app.key_input.trim().to_string();
    if key.is_empty() || app.auth_busy {
        return;
    }
    app.error = None;
    app.auth_busy = true;
    app.auth_status = "validando…".into();
    spawn_finish(key, tx.clone());
}

fn handle_key(app: &mut App, code: KeyCode, tx: &mpsc::UnboundedSender<app::Msg>) {
    match app.screen {
        Screen::KeyEntry => {
            // Reconexión/OAuth en curso: solo Esc cancela.
            if app.auth_busy {
                if code == KeyCode::Esc {
                    app.should_quit = true;
                }
                return;
            }
            if app.paste_mode {
                match code {
                    KeyCode::Esc => app.should_quit = true,
                    KeyCode::Enter => submit_key(app, tx),
                    KeyCode::Backspace => {
                        app.key_input.pop();
                    }
                    KeyCode::Char(c) if !c.is_whitespace() => app.key_input.push(c),
                    _ => {}
                }
            } else {
                // Pantalla de elección: OAuth (enter) o pegar llave (k).
                match code {
                    KeyCode::Enter => {
                        app.error = None;
                        app.auth_busy = true;
                        app.auth_status = "abriendo el navegador…".into();
                        spawn_oauth(tx.clone());
                    }
                    KeyCode::Char('k') | KeyCode::Char('K') => {
                        app.error = None;
                        app.paste_mode = true;
                    }
                    KeyCode::Esc => app.should_quit = true,
                    _ => {}
                }
            }
        }
        Screen::Apps => {
            let n = app.apps.len();
            // Confirmación de borrado pendiente: s = sí, cualquier otra = cancela.
            if app.confirm_destroy {
                app.confirm_destroy = false;
                if matches!(code, KeyCode::Char('s' | 'S' | 'y' | 'Y')) && n > 0 {
                    let id = app.apps[app.apps_cursor].id.clone();
                    if let Some(client) = app.client.clone() {
                        app.busy = Some("borrando…".into());
                        spawn_destroy_and_reload(client, id, app.email.clone(), tx.clone());
                    }
                }
                return;
            }
            // Crear: tecla `c`, o `enter` cuando no hay apps (nada que seleccionar).
            let create = matches!(code, KeyCode::Char('c') | KeyCode::Char('C'))
                || (code == KeyCode::Enter && n == 0);
            if create {
                app.key_input.clear();
                app.logo_input.clear();
                app.accent_idx = 0;
                app.custom_hex = "#".into();
                app.focus = 0;
                app.screen = Screen::Consent;
                return;
            }
            match code {
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                KeyCode::Up if n > 0 => app.apps_cursor = (app.apps_cursor + n - 1) % n,
                KeyCode::Down if n > 0 => app.apps_cursor = (app.apps_cursor + 1) % n,
                KeyCode::Enter if n > 0 => {
                    let a = &app.apps[app.apps_cursor];
                    app.url = Some(a.url.clone());
                    app.sandbox_id = Some(a.id.clone());
                    app.live_at = None; // ver existente → sin confetti
                    app.screen = Screen::Live;
                }
                KeyCode::Char('o') if n > 0 => {
                    let _ = open_browser(&app.apps[app.apps_cursor].url);
                }
                KeyCode::Char('d') if n > 0 => app.confirm_destroy = true,
                _ => {}
            }
        }
        Screen::Consent => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.key_input.clear(); // se reusa como buffer del nombre
                app.screen = Screen::Customize;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.screen = Screen::Apps;
            }
            _ => {}
        },
        Screen::Customize => {
            let count = app::ACCENTS.len() + 1; // presets + custom
            match code {
                KeyCode::Esc => app.screen = Screen::Apps,
                KeyCode::Tab | KeyCode::Down => app.focus = (app.focus + 1) % app::FOCUS_COUNT,
                KeyCode::Up => {
                    app.focus = (app.focus + app::FOCUS_COUNT - 1) % app::FOCUS_COUNT;
                }
                // Enter AVANZA entre pasos; solo publica en el paso "Publicar".
                // (Evita lanzar vacío por error.)
                KeyCode::Enter if app.focus != app::FOCUS_PUBLISH => {
                    app.focus += 1;
                }
                KeyCode::Enter => {
                    if let Some(client) = app.client.clone() {
                        app.app_name = app.key_input.clone();
                        let accent = app::chosen_accent(app);
                        let logo = app.logo_input.clone();
                        app.start_launch();
                        spawn_launch(client, tx.clone(), app.app_name.clone(), accent, logo);
                    }
                }
                // Fila de color.
                _ if app.focus == app::FOCUS_COLOR => match code {
                    KeyCode::Left => app.accent_idx = (app.accent_idx + count - 1) % count,
                    KeyCode::Right => app.accent_idx = (app.accent_idx + 1) % count,
                    KeyCode::Backspace if app.accent_idx == app::CUSTOM_ACCENT => {
                        app.custom_hex.pop();
                    }
                    KeyCode::Char(c)
                        if app.accent_idx == app::CUSTOM_ACCENT
                            && (c == '#' || c.is_ascii_hexdigit())
                            && app.custom_hex.chars().count() < 7 =>
                    {
                        app.custom_hex.push(c);
                    }
                    _ => {}
                },
                // Campo logo (ruta/URL).
                _ if app.focus == app::FOCUS_LOGO => match code {
                    KeyCode::Backspace => {
                        app.logo_input.pop();
                    }
                    KeyCode::Char(c) => app.logo_input.push(c),
                    _ => {}
                },
                // Campo nombre.
                KeyCode::Backspace => {
                    app.key_input.pop();
                }
                KeyCode::Char(c) if app.key_input.chars().count() < 40 => {
                    app.key_input.push(c);
                }
                _ => {}
            }
        }
        Screen::Launching => {
            if code == KeyCode::Esc {
                app.should_quit = true;
            }
        }
        Screen::Live => {
            // Confirmación de borrado pendiente.
            if app.confirm_destroy {
                app.confirm_destroy = false;
                if matches!(code, KeyCode::Char('s' | 'S' | 'y' | 'Y')) {
                    if let (Some(client), Some(id)) = (app.client.clone(), app.sandbox_id.clone()) {
                        app.busy = Some("borrando…".into());
                        app.screen = Screen::Apps; // el indicador se ve en el panel
                        spawn_destroy_and_reload(client, id, app.email.clone(), tx.clone());
                    }
                }
                return;
            }
            match code {
                KeyCode::Char('q') => app.should_quit = true,
                KeyCode::Char('o') => {
                    if let Some(url) = &app.url {
                        let _ = open_browser(url);
                    }
                }
                // Volver al panel: instantáneo (lista en memoria) + refresca al fondo.
                KeyCode::Char('b') | KeyCode::Esc => {
                    app.screen = Screen::Apps;
                    if let Some(client) = app.client.clone() {
                        app.busy = Some("actualizando…".into());
                        spawn_list_apps(client, app.email.clone(), tx.clone());
                    }
                }
                KeyCode::Char('d') => app.confirm_destroy = true,
                _ => {}
            }
        }
        Screen::Error => {
            if code == KeyCode::Esc {
                app.should_quit = true;
            }
        }
    }
}

fn open_browser(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    std::process::Command::new(cmd).arg(url).spawn().map(|_| ())
}
