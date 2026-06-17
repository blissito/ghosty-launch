//! Audio ambiental no invasivo. Un *drone* fantasmal grave mientras Ghosty trabaja
//! (publicando / agente), y un chime al quedar Live. Sintetizado con `rodio` — sin
//! archivos. `rodio` es `!Send`, así que el `OutputStream` vive en un thread dedicado
//! que escucha comandos por canal; el TUI solo manda mensajes.
//!
//! Silenciar: `GHOSTY_NO_AUDIO=1` (desactiva todo) o la tecla `m` en runtime.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

enum Cmd {
    Ambient,
    Stop,
    Chime,
}

#[derive(Clone)]
pub struct Audio {
    tx: Option<Sender<Cmd>>,
}

impl Audio {
    pub fn new() -> Self {
        if std::env::var_os("GHOSTY_NO_AUDIO").is_some() {
            return Self { tx: None };
        }
        let (tx, rx) = mpsc::channel::<Cmd>();
        thread::spawn(move || run_audio(rx));
        Self { tx: Some(tx) }
    }

    /// Arranca (idempotente) el drone ambiental.
    pub fn ambient(&self) {
        self.send(Cmd::Ambient);
    }
    /// Corta el ambiente.
    pub fn stop(&self) {
        self.send(Cmd::Stop);
    }
    /// Suena el chime de "listo" (al quedar Live).
    pub fn chime(&self) {
        self.send(Cmd::Chime);
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(c);
        }
    }
}

fn run_audio(rx: mpsc::Receiver<Cmd>) {
    // El OutputStream debe vivir en este thread (es !Send). Si no hay dispositivo de
    // audio, drenamos los comandos sin hacer nada (jamás crashea por audio).
    let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
        for _ in rx {}
        return;
    };

    let mut ambient: Option<rodio::Sink> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Ambient => {
                if ambient.is_some() {
                    continue; // ya sonando
                }
                if let Ok(sink) = rodio::Sink::try_new(&handle) {
                    use rodio::Source;
                    // Dos graves ligeramente desafinados → beating lento (sensación
                    // "viva"), + una quinta tenue. Muy bajo: ambiente, no protagonista.
                    let drone = rodio::source::SineWave::new(98.0)
                        .mix(rodio::source::SineWave::new(98.6))
                        .mix(rodio::source::SineWave::new(147.0).amplify(0.5))
                        .amplify(0.07);
                    sink.append(drone);
                    ambient = Some(sink);
                }
            }
            Cmd::Stop => {
                if let Some(s) = ambient.take() {
                    s.stop();
                }
            }
            Cmd::Chime => {
                if let Ok(sink) = rodio::Sink::try_new(&handle) {
                    use rodio::Source;
                    // Arpegio ascendente A mayor — "listo 🟢".
                    for f in [440.0f32, 554.37, 659.25, 880.0] {
                        let note = rodio::source::SineWave::new(f)
                            .take_duration(Duration::from_millis(150))
                            .fade_in(Duration::from_millis(15))
                            .amplify(0.18);
                        sink.append(note);
                    }
                    sink.detach(); // suena hasta el final por su cuenta
                }
            }
        }
    }
}
