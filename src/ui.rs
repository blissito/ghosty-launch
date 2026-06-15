//! Render del TUI con ratatui.
//! Estética: fantasmita Ghosty (el del chat de ghostycode), morado de marca,
//! tarjeta centrada con borde redondeado, animaciones sutiles.

use crate::app::{App, Screen, StepStatus};
use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, LineGauge, Padding, Paragraph, Wrap},
    Frame,
};

// Paleta — morado Ghosty muestreado de la marca (#A29BE8), igual que ghostycode.
const ACCENT: Color = Color::Rgb(162, 155, 232);
const ACCENT_SOFT: Color = Color::Rgb(120, 113, 190);
const TEXT: Color = Color::Rgb(230, 230, 236);
const DIM: Color = Color::Rgb(92, 92, 108);
const SUCCESS: Color = Color::Rgb(122, 211, 161);
const ERROR: Color = Color::Rgb(245, 110, 130);

// Fantasmita de bloque (el de adentro del chat de ghostycode). Ojos = `eye`.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    let (title, content) = match app.screen {
        Screen::KeyEntry => ("conexión", key_entry(app)),
        Screen::Apps => ("tus apps", apps(app)),
        Screen::Consent => ("consentimiento", consent(app)),
        Screen::Customize => ("personaliza", customize(app)),
        Screen::Launching => ("publicando", launch(app)),
        Screen::Live => ("en vivo", launch(app)),
        Screen::Error => ("error", error(app)),
    };

    draw_card(frame, rows[0], app, title, content);
    draw_footer(frame, rows[1], app);
}

/// Tarjeta centrada (vert + horiz) con fantasmita hero arriba y contenido abajo.
fn draw_card(frame: &mut Frame, area: Rect, app: &App, title: &str, content: Vec<Line<'static>>) {
    let hero_h = 5u16; // 3 mascot + 1 wordmark + 1 espacio
                       // Ancho adaptativo al contenido (la URL live puede ser larga) para no truncar.
                       // chrome = bordes (2) + padding izq/der (4).
    let content_w = content.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    let max_w = area.width.saturating_sub(2);
    let min_w = 56u16.min(max_w); // en terminales angostas, min no puede exceder max
    let card_w = (content_w + 6).clamp(min_w, max_w);
    let inner_w = card_w.saturating_sub(6).max(1);
    // Filas extra por wrap: la URL gris informativa baja de línea en vez de
    // cortarse; reservamos su altura para no clipear.
    let extra: u16 = content
        .iter()
        .map(|l| (l.width() as u16).saturating_sub(1) / inner_w)
        .sum();
    let inner_h = hero_h + content.len() as u16 + extra;
    let card_h = (inner_h + 4).min(area.height.saturating_sub(1)); // +bordes +padding

    let card = center(area, card_w, card_h);
    frame.render_widget(Clear, card);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT_SOFT))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(card);
    frame.render_widget(block, card);

    let split = Layout::vertical([Constraint::Length(hero_h), Constraint::Min(0)]).split(inner);
    frame.render_widget(
        Paragraph::new(hero(app)).alignment(ratatui::layout::Alignment::Center),
        split[0],
    );
    frame.render_widget(Paragraph::new(content).wrap(Wrap { trim: false }), split[1]);

    // Barra de progreso al fondo de la tarjeta en la pantalla de publicación.
    if app.screen == Screen::Launching {
        let done = app
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .count();
        let ratio = if app.steps.is_empty() {
            0.0
        } else {
            done as f64 / app.steps.len() as f64
        };
        let gauge_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            LineGauge::default()
                .filled_style(Style::default().fg(ACCENT))
                .unfilled_style(Style::default().fg(DIM))
                .ratio(ratio),
            gauge_area,
        );
    }

    // Hyperlink OSC 8 clickeable sobre el placeholder (fila tras "● tu app está live").
    if app.screen == Screen::Live {
        if let Some(url) = &app.url {
            let row = split[1].y + app.steps.len() as u16 + 2;
            if row < inner.bottom() {
                render_hyperlink(frame.buffer_mut(), split[1].x, row, "→ abrir tu app ↗", url);
            }
        }
    }
}

/// Fantasmita Ghosty (3 líneas) + wordmark, en morado. Parpadea y cambia ojos.
fn hero(app: &App) -> Vec<Line<'static>> {
    let blink = app.tick % 55 >= 53;
    let eye = if blink {
        "─"
    } else {
        match app.screen {
            Screen::Live => "◕",
            Screen::Error => "×",
            _ => "◑",
        }
    };
    let mascot = [
        " ▄████▄ ".to_string(),
        format!("▐ {eye}  {eye} ▌"),
        "▐█▀██▀█▌".to_string(),
    ];
    // Mientras publica, rodeamos al fantasma de destellos que titilan.
    let working = app.screen == Screen::Launching;
    let mut out: Vec<Line<'static>> = Vec::new();
    if working {
        out.push(Line::from(spark_strip(app.tick, 100, 22)));
    }
    for (r, row) in mascot.into_iter().enumerate() {
        if working {
            let mut spans = spark_strip(app.tick, r as i64 * 7, 5);
            spans.push(Span::raw("  "));
            spans.push(Span::styled(row, Style::default().fg(ACCENT)));
            spans.push(Span::raw("  "));
            spans.extend(spark_strip(app.tick, r as i64 * 7 + 60, 5));
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(Span::styled(row, Style::default().fg(ACCENT))));
        }
    }
    // Wordmark con shimmer: gradiente RGB por carácter que viaja con el tick.
    let mut wordmark = shimmer("Ghosty Launch", app.tick, Modifier::BOLD);
    wordmark.push(Span::styled(
        "  ·  que lo haga Ghosty",
        Style::default().fg(DIM),
    ));
    out.push(Line::from(wordmark));
    out
}

/// Gradiente animado por carácter (una onda de brillo que recorre el texto).
/// Showcase del render por-celda de ratatui: cada glifo es su propio Span con su color.
fn shimmer(text: &str, tick: u64, extra: Modifier) -> Vec<Span<'static>> {
    let lo = (120u8, 110, 200); // morado profundo
    let hi = (214u8, 196, 255); // lavanda brillante
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let phase = i as f32 * 0.55 - tick as f32 * 0.22;
            let w = 0.5 + 0.5 * phase.sin();
            let c = Color::Rgb(
                lerp(lo.0, hi.0, w),
                lerp(lo.1, hi.1, w),
                lerp(lo.2, hi.2, w),
            );
            Span::styled(ch.to_string(), Style::default().fg(c).add_modifier(extra))
        })
        .collect()
}

/// Tira de destellos que titilan y se desplazan con el tick (Ghosty "trabajando").
/// `seed` desfasa cada tira para que las de alrededor del fantasma no estén en sync.
fn spark_strip(tick: u64, seed: i64, width: i64) -> Vec<Span<'static>> {
    const GLYPHS: [char; 4] = ['·', '✦', '✧', '✦'];
    let t = tick as i64;
    (0..width)
        .map(|x| {
            let col = x + seed;
            let phase = (col * 5 + t).rem_euclid(29);
            if phase < 3 {
                let g = GLYPHS[((col + t / 3).rem_euclid(GLYPHS.len() as i64)) as usize];
                let color = if phase == 1 { ACCENT } else { ACCENT_SOFT };
                Span::styled(g.to_string(), Style::default().fg(color))
            } else {
                Span::raw(" ")
            }
        })
        .collect()
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).clamp(0.0, 255.0) as u8
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let chips: &[(&str, &str)] = match app.screen {
        Screen::KeyEntry if app.auth_busy => &[("esc", "cancelar")],
        Screen::KeyEntry if app.paste_mode => &[("enter", "validar"), ("esc", "salir")],
        Screen::KeyEntry => &[
            ("enter", "conectar"),
            ("k", "pegar llave"),
            ("esc", "salir"),
        ],
        Screen::Apps if app.apps.is_empty() => &[("enter", "publicar"), ("q", "salir")],
        Screen::Apps => &[
            ("enter", "ver"),
            ("c", "crear"),
            ("d", "borrar"),
            ("q", "salir"),
        ],
        Screen::Consent => &[("y", "publicar"), ("n", "volver")],
        Screen::Customize => &[("enter", "siguiente"), ("⇥", "campo"), ("esc", "volver")],
        Screen::Launching => &[("esc", "cancelar")],
        Screen::Live => &[
            ("o", "abrir"),
            ("b", "volver"),
            ("d", "destruir"),
            ("q", "salir"),
        ],
        Screen::Error => &[("esc", "salir")],
    };

    // En live: un hyperlink OSC 8 clickeable ("→ abrir ↗") cuyo destino es la URL
    // completa — corto, no se trunca, y el click abre bien aunque la URL sea larga.
    if app.screen == Screen::Live {
        if let Some(url) = app.url.clone() {
            let label = "  → abrir tu app ↗";
            render_hyperlink(frame.buffer_mut(), area.x, area.y, label, &url);
            let off = label.chars().count() as u16 + 3;
            let rest = Rect {
                x: area.x + off.min(area.width),
                y: area.y,
                width: area.width.saturating_sub(off),
                height: 1,
            };
            frame.render_widget(Paragraph::new(Line::from(chip_spans(chips))), rest);
            return;
        }
    }

    let mut spans = vec![Span::raw("  ")];
    spans.extend(chip_spans(chips));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn chip_spans(chips: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (k, label) in chips {
        spans.push(Span::styled(format!(" {k} "), Style::default().fg(ACCENT)));
        spans.push(Span::styled(
            format!("{label}   "),
            Style::default().fg(DIM),
        ));
    }
    spans
}

/// Escribe un hyperlink OSC 8 directamente en el buffer: la secuencia completa va
/// en la primera celda y las siguientes se marcan `skip` para que la terminal
/// renderice el texto sobre ellas (técnica manual estándar; `tui-link` usa la misma
/// idea vía un PR de ratatui aún no estable). Sin soporte OSC 8 se ve el texto plano.
fn render_hyperlink(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, display: &str, url: &str) {
    if x >= buf.area.right() || y >= buf.area.bottom() {
        return;
    }
    let seq = format!("\x1b]8;;{url}\x1b\\{display}\x1b]8;;\x1b\\");
    buf[(x, y)].set_symbol(&seq).set_style(
        Style::default()
            .fg(ACCENT)
            .add_modifier(Modifier::UNDERLINED),
    );
    let w = display.chars().count() as u16;
    let mut dx = 1;
    while dx < w && x + dx < buf.area.right() {
        buf[(x + dx, y)].set_skip(true);
        dx += 1;
    }
}

fn key_entry(app: &App) -> Vec<Line<'static>> {
    let title = Line::from(Span::styled(
        "Conéctate con EasyBits",
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
    ));

    // Reconexión / OAuth en curso.
    if app.auth_busy {
        let sp = SPINNER[(app.tick % 10) as usize];
        return vec![
            title,
            Line::from(""),
            Line::from(vec![
                Span::styled(format!("{sp} "), Style::default().fg(ACCENT)),
                Span::styled(app.auth_status.clone(), Style::default().fg(ACCENT)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "autoriza en la pestaña del navegador",
                Style::default().fg(DIM),
            )),
        ];
    }

    // Pegar llave manualmente.
    if app.paste_mode {
        let cursor = if (app.tick / 8).is_multiple_of(2) {
            "▏"
        } else {
            " "
        };
        let status = if app.validating {
            Span::styled(
                format!("{} validando…", SPINNER[(app.tick % 10) as usize]),
                Style::default().fg(ACCENT),
            )
        } else if let Some(err) = &app.error {
            Span::styled(err.clone(), Style::default().fg(ERROR))
        } else {
            Span::styled(
                "pega tu llave eb_sk_… (valida sola)",
                Style::default().fg(DIM),
            )
        };
        return vec![
            Line::from(Span::styled(
                "Tu llave de EasyBits",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("› ", Style::default().fg(ACCENT)),
                Span::styled(mask_key(&app.key_input), Style::default().fg(TEXT)),
                Span::styled(cursor.to_string(), Style::default().fg(ACCENT)),
            ]),
            Line::from(""),
            Line::from(status),
        ];
    }

    // Elección.
    let opt = |key: &str, label: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {key}  "),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(label.to_string(), Style::default().fg(TEXT)),
        ])
    };
    let mut out = vec![
        title,
        Line::from(""),
        opt("enter", "conectar con el navegador (OAuth)"),
        opt("k", "pegar tu llave eb_sk_…"),
        Line::from(""),
    ];
    out.push(if let Some(err) = &app.error {
        Line::from(Span::styled(err.clone(), Style::default().fg(ERROR)))
    } else {
        Line::from(Span::styled(
            "sin copiar llaves: te conecta por el navegador",
            Style::default().fg(DIM),
        ))
    });
    out
}

fn consent(app: &App) -> Vec<Line<'static>> {
    let who = app.email.clone().unwrap_or_else(|| "tu cuenta".to_string());
    let step = |t: &str| {
        Line::from(vec![
            Span::styled("  · ", Style::default().fg(ACCENT)),
            Span::styled(t.to_string(), Style::default().fg(TEXT)),
        ])
    };
    vec![
        Line::from(Span::styled(
            format!("✓ conectado: {who}"),
            Style::default().fg(SUCCESS),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Ghosty hará por ti:",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        step("levantar una VM tuya en EasyBits"),
        step("clonar tu repo e instalar dependencias"),
        step("arrancar la app y publicar su URL pública"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "¿Publicamos?",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   y / n", Style::default().fg(DIM)),
        ]),
    ]
}

fn apps(app: &App) -> Vec<Line<'static>> {
    if app.apps.is_empty() {
        return vec![
            Line::from(Span::styled(
                "Aún no tienes apps publicadas.",
                Style::default().fg(TEXT),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  enter  ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled("publicar una", Style::default().fg(TEXT)),
            ]),
        ];
    }
    let mut out = vec![
        Line::from(Span::styled(
            format!("Tus apps ({})", app.apps.len()),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (i, a) in app.apps.iter().enumerate() {
        let sel = i == app.apps_cursor;
        let name_style = if sel {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        out.push(Line::from(vec![
            Span::styled(
                if sel { "› ● " } else { "  ● " }.to_string(),
                Style::default().fg(if sel { ACCENT } else { SUCCESS }),
            ),
            Span::styled(a.name.clone(), name_style),
        ]));
        if sel {
            out.push(Line::from(Span::styled(
                format!("      {}", a.url),
                Style::default().fg(DIM),
            )));
        }
    }
    if app.confirm_destroy {
        let name = app
            .apps
            .get(app.apps_cursor)
            .map(|a| a.name.clone())
            .unwrap_or_default();
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            format!("¿Borrar «{name}»?  s = sí · esc = no"),
            Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
        )));
    }
    out
}

fn customize(app: &App) -> Vec<Line<'static>> {
    let name_focus = app.focus == crate::app::FOCUS_NAME;
    let color_focus = app.focus == crate::app::FOCUS_COLOR;
    let logo_focus = app.focus == crate::app::FOCUS_LOGO;
    let blink = (app.tick / 8).is_multiple_of(2);
    let marker = |on: bool| {
        if on {
            Span::styled("› ", Style::default().fg(ACCENT))
        } else {
            Span::raw("  ")
        }
    };

    // Fila nombre.
    let name_cursor = if name_focus && blink { "▏" } else { " " };
    let name_line = Line::from(vec![
        marker(name_focus),
        Span::styled("nombre  ", Style::default().fg(DIM)),
        Span::styled(app.key_input.clone(), Style::default().fg(TEXT)),
        Span::styled(name_cursor.to_string(), Style::default().fg(ACCENT)),
    ]);

    // Fila color: presets + opción custom (✎).
    let mut sw: Vec<Span<'static>> = vec![
        marker(color_focus),
        Span::styled("color   ", Style::default().fg(DIM)),
    ];
    for (i, (_, (r, g, b))) in crate::app::ACCENTS.iter().enumerate() {
        let col = Color::Rgb(*r, *g, *b);
        if i == app.accent_idx {
            sw.push(Span::styled("[", Style::default().fg(TEXT)));
            sw.push(Span::styled("█", Style::default().fg(col)));
            sw.push(Span::styled("]", Style::default().fg(TEXT)));
        } else {
            sw.push(Span::styled(" █ ", Style::default().fg(col)));
        }
    }
    let custom_col = parse_hex(&app.custom_hex).unwrap_or(DIM);
    if app.accent_idx == crate::app::CUSTOM_ACCENT {
        sw.push(Span::styled("[", Style::default().fg(TEXT)));
        sw.push(Span::styled("✎", Style::default().fg(custom_col)));
        sw.push(Span::styled("]", Style::default().fg(TEXT)));
    } else {
        sw.push(Span::styled(" ✎ ", Style::default().fg(DIM)));
    }

    let mut out = vec![
        Line::from(Span::styled(
            "Personaliza tu app",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        name_line,
        Line::from(""),
        Line::from(sw),
    ];

    // Editor de hex cuando "custom" está activo.
    if app.accent_idx == crate::app::CUSTOM_ACCENT {
        let hex_cursor = if color_focus && blink { "▏" } else { " " };
        out.push(Line::from(vec![
            Span::raw("        "),
            Span::styled(app.custom_hex.clone(), Style::default().fg(custom_col)),
            Span::styled(hex_cursor.to_string(), Style::default().fg(ACCENT)),
            Span::styled("  (#rrggbb)", Style::default().fg(DIM)),
        ]));
    }

    // Fila logo (ruta o URL; arrastra el archivo).
    out.push(Line::from(""));
    let logo_cursor = if logo_focus && blink { "▏" } else { " " };
    let logo_val = if app.logo_input.is_empty() {
        Span::styled(
            "(opcional — ruta o URL, o arrastra)",
            Style::default().fg(DIM),
        )
    } else {
        Span::styled(app.logo_input.clone(), Style::default().fg(TEXT))
    };
    out.push(Line::from(vec![
        marker(logo_focus),
        Span::styled("logo    ", Style::default().fg(DIM)),
        logo_val,
        Span::styled(logo_cursor.to_string(), Style::default().fg(ACCENT)),
    ]));

    // Paso final: botón Publicar (solo aquí Enter lanza).
    out.push(Line::from(""));
    let pub_focus = app.focus == crate::app::FOCUS_PUBLISH;
    let pub_style = if pub_focus {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    out.push(Line::from(vec![
        marker(pub_focus),
        Span::styled("▸ Publicar", pub_style),
    ]));

    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "↑↓/⇥ moverse · enter avanza · en Publicar enter lanza",
        Style::default().fg(DIM),
    )));
    out
}

fn parse_hex(s: &str) -> Option<Color> {
    let h = crate::app::normalize_hex(s)?;
    let h = h.trim_start_matches('#');
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn launch(app: &App) -> Vec<Line<'static>> {
    let frame = SPINNER[(app.tick % 10) as usize];
    let mut out: Vec<Line<'static>> = Vec::new();

    for step in &app.steps {
        let (icon, color) = match step.status {
            StepStatus::Pending => ("○".to_string(), DIM),
            StepStatus::Running => (frame.to_string(), ACCENT),
            StepStatus::Done => ("●".to_string(), SUCCESS),
            StepStatus::Failed => ("✕".to_string(), ERROR),
        };
        let label_style = match step.status {
            StepStatus::Pending => Style::default().fg(DIM),
            _ => Style::default().fg(TEXT),
        };
        out.push(Line::from(vec![
            Span::styled(format!("{icon} "), Style::default().fg(color)),
            Span::styled(step.label.clone(), label_style),
        ]));
        if !step.detail.is_empty() && step.status != StepStatus::Done {
            out.push(Line::from(Span::styled(
                format!("   {}", truncate(&step.detail, 52)),
                Style::default().fg(DIM),
            )));
        }
    }
    if let Some(url) = &app.url {
        out.push(Line::from(""));
        out.push(Line::from(shimmer(
            "● tu app está en vivo",
            app.tick,
            Modifier::BOLD,
        )));
        // Placeholder: el hyperlink OSC 8 clickeable se pinta encima en draw_card
        // (texto corto → no se trunca, el click abre la URL completa).
        out.push(Line::from(""));
        // URL completa, plana y atenuada — solo de referencia (se recorta limpio
        // en ventana angosta, sin desbordar el borde).
        out.push(Line::from(Span::styled(
            url.clone(),
            Style::default().fg(DIM),
        )));
        if let Some(id) = &app.sandbox_id {
            let short: String = id
                .strip_prefix("sb_")
                .unwrap_or(id)
                .chars()
                .take(8)
                .collect();
            let repo = App::ref_repo();
            let repo_short = repo
                .trim_start_matches("https://")
                .trim_start_matches("github.com/")
                .trim_end_matches(".git");
            out.push(Line::from(""));
            out.push(Line::from(Span::styled(
                format!("repo: {repo_short}"),
                Style::default().fg(DIM),
            )));
            out.push(Line::from(Span::styled(
                format!("clonado en /app dentro de tu VM ({short}…)"),
                Style::default().fg(DIM),
            )));
        }
        if app.confirm_destroy {
            out.push(Line::from(""));
            out.push(Line::from(Span::styled(
                "¿Borrar esta app?  s = sí · esc = no",
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            )));
        }
    } else {
        out.push(Line::from(""));
    }
    out
}

fn error(app: &App) -> Vec<Line<'static>> {
    let err = app
        .error
        .clone()
        .unwrap_or_else(|| "error desconocido".to_string());
    vec![
        Line::from(Span::styled(
            "Algo falló",
            Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(err, Style::default().fg(TEXT))),
    ]
}

/// Rect centrado vertical + horizontalmente.
fn center(area: Rect, w: u16, h: u16) -> Rect {
    let [h_area] = Layout::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(area);
    let [out] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(h_area);
    out
}

fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= 8 {
        return "•".repeat(n);
    }
    let prefix: String = key.chars().take(6).collect();
    format!("{prefix}{}", "•".repeat(n - 6))
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() > max {
        let tail: String = s.chars().rev().take(max).collect();
        format!("…{}", tail.chars().rev().collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod snapshot {
    use super::*;
    use crate::app::{App, AppEntry, Screen, StepStatus};
    use ratatui::{backend::TestBackend, Terminal};

    fn render_str(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..h {
            let mut line = String::new();
            for x in 0..w {
                line.push_str(buf[(x, y)].symbol());
            }
            s.push_str(line.trim_end());
            s.push('\n');
        }
        s
    }

    #[test]
    fn key_entry() {
        let mut app = App::new();
        app.key_input = "eb_sk_live_abc123def456".into();
        insta::assert_snapshot!(render_str(&app, 78, 20));
    }

    #[test]
    fn consent() {
        let mut app = App::new();
        app.email = Some("fixtergeek@gmail.com".into());
        app.screen = Screen::Consent;
        insta::assert_snapshot!(render_str(&app, 78, 20));
    }

    #[test]
    fn apps_panel() {
        let mut app = App::new();
        app.screen = Screen::Apps;
        app.apps = vec![
            AppEntry {
                id: "sb_a".into(),
                name: "Mi Tienda".into(),
                url: "https://sb-a-3000.sandboxes.easybits.cloud".into(),
            },
            AppEntry {
                id: "sb_b".into(),
                name: "Blog".into(),
                url: "https://sb-b-3000.sandboxes.easybits.cloud".into(),
            },
        ];
        insta::assert_snapshot!(render_str(&app, 78, 22));
    }

    #[test]
    fn customize() {
        let mut app = App::new();
        app.screen = Screen::Customize;
        app.key_input = "Mi Tienda".into();
        app.accent_idx = 2;
        insta::assert_snapshot!(render_str(&app, 78, 20));
    }

    #[test]
    fn launching() {
        let mut app = App::new();
        app.start_launch();
        app.steps[0].status = StepStatus::Done;
        app.steps[1].status = StepStatus::Running;
        insta::assert_snapshot!(render_str(&app, 78, 20));
    }

    #[test]
    fn live() {
        let mut app = App::new();
        app.start_launch();
        for s in app.steps.iter_mut() {
            s.status = StepStatus::Done;
        }
        app.url = Some("https://sb-abc123-3000.sandboxes.easybits.cloud".into());
        app.sandbox_id = Some("sb_abc12345-6789".into());
        app.screen = Screen::Live;
        // live_at = None → URL totalmente revelada (snapshot determinista).
        insta::assert_snapshot!(render_str(&app, 78, 22));
    }
}
