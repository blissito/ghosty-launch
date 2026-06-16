//! Tool-support helpers vendored from `ghostycode/crates/tui/src/tools/`.
//!
//! Only the pieces the vendored client touches are brought over:
//!   - `schema_sanitize::sanitize_for_kimi` — Moonshot/Kimi JSON-Schema fixup
//!   - `truncate` — SHA-addressed tool-result spillover (write + path helpers)

pub mod schema_sanitize;
pub mod truncate;
