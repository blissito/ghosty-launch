//! Bips & blips: micro-sonidos discretos al iniciar/terminar acciones. Antes había un
//! drone ambiental continuo, pero cansaba; se quitó. Ahora solo efectos cortos
//! sintetizados con `rodio` — sin archivos. `rodio` es `!Send`, así que el `OutputStream`
//! vive en un thread dedicado que escucha comandos por canal; el TUI solo manda mensajes.
//!
//! Silenciar: `GHOSTY_NO_AUDIO=1` (desactiva todo) o la tecla `m` en runtime.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

enum Cmd {
    Boot,
    Start,
    Done,
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

    /// Blip cálido y breve al abrir la app.
    pub fn boot(&self) {
        self.send(Cmd::Boot);
    }
    /// "Tu-tip" ascendente: empieza una acción (deploy, agente).
    pub fn start(&self) {
        self.send(Cmd::Start);
    }
    /// "Tip-tu" descendente: una acción terminó (sin quedar Live).
    pub fn done(&self) {
        self.send(Cmd::Done);
    }
    /// Arpegio "listo 🟢" al quedar Live.
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

    // Cada comando es un disparo único (una secuencia corta de notas). No hay nada
    // continuo que mantener entre comandos, así que ni rastreamos el sink: se detacha
    // y suena hasta el final por su cuenta.
    while let Ok(cmd) = rx.recv() {
        // (frecuencia Hz, duración ms) por nota + amplitud de la secuencia.
        let (notes, amp): (&[(f32, u64)], f32) = match cmd {
            Cmd::Boot => (&[(660.0, 130)], 0.13),
            Cmd::Start => (&[(587.33, 70), (880.0, 95)], 0.13),
            Cmd::Done => (&[(660.0, 70), (440.0, 95)], 0.13),
            Cmd::Chime => (
                &[(440.0, 150), (554.37, 150), (659.25, 150), (880.0, 150)],
                0.18,
            ),
        };
        if let Ok(sink) = rodio::Sink::try_new(&handle) {
            use rodio::Source;
            for &(f, ms) in notes {
                // fade_in corto en cada nota → sin clicks de arranque.
                let note = rodio::source::SineWave::new(f)
                    .take_duration(Duration::from_millis(ms))
                    .fade_in(Duration::from_millis(12))
                    .amplify(amp);
                sink.append(note);
            }
            sink.detach(); // suena hasta el final por su cuenta
        }
    }
}
