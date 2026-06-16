//! Motor LLM DeepSeek vendorizado de ghostycode (read-only; copiado, no compartido).
//!
//! Mantiene la lógica fina de DeepSeek que pone a v4-pro en la liga de Sonnet:
//! replay de `reasoning_content`, JSON del assistant byte-idéntico para cache caliente,
//! mapeo de `reasoning_effort` por provider, parseo de `tool_calls`/`usage`.
//!
//! Módulos mirroreados de `ghostycode/crates/tui/src/` para que los paths `crate::…`
//! resuelvan sin reescritura. Subset de `config` extraído (no el monstruo de 10k líneas).
//!
//! Código VENDORIZADO (copiado de ghostycode): no policeamos sus lints. Parte de la API
//! (cache-warmup, etc.) no la usa el loop no-streaming de launch todavía.
#![allow(dead_code, unused_imports)]

pub mod client;
pub mod config;
pub mod llm_client;
pub mod logging;
pub mod models;
pub mod pricing;
pub mod retry_status;
pub mod tools;
pub mod turn;

pub use turn::complete_turn;
