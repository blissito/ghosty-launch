//! Override local de la receta de deploy — la zona durable donde el **agente**
//! aterriza un arreglo.
//!
//! El `ghosty.toml` del repo (ver [`crate::app`] `fetch_manifest`) vive en GitHub y el
//! agente no puede escribirlo. Pero el arreglo tiene que sobrevivir al próximo deploy
//! (que hace `git clone` fresco y borra el VM). Solución: launch guarda un override
//! local por app en `~/.config/ghosty-launch/overrides/<app>.toml`, y el deploy lo
//! fusiona ENCIMA del manifiesto del repo. Así el fix queda durable sin tocar el repo
//! del usuario.
//!
//! Mismo patrón de path que las credenciales en [`crate::oauth`].

use std::path::PathBuf;

/// Lo que el agente puede fijar de forma durable, por app. Todo opcional: lo que falte
/// cae al `ghosty.toml` del repo y, si tampoco está, al auto-detect del deploy.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Override {
    /// Envs persistentes (el caso típico: una connection string que faltaba).
    #[serde(default)]
    pub envs: Vec<(String, String)>,
    /// Comando de arranque (gana sobre `[deploy] start` del repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Paso de build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Comando de install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<String>,
}

fn overrides_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    Some(base.join("ghosty-launch").join("overrides"))
}

fn path_for(app: &str) -> Option<PathBuf> {
    Some(overrides_dir()?.join(format!("{app}.toml")))
}

/// Carga el override de `app`. Sin archivo (o ilegible) → `Override::default()`.
pub fn load(app: &str) -> Override {
    path_for(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persiste el override de `app`. Devuelve `false` si no se pudo escribir.
pub fn save(app: &str, ovr: &Override) -> bool {
    let Some(path) = path_for(app) else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match toml::to_string_pretty(ovr) {
        Ok(s) => std::fs::write(&path, s).is_ok(),
        Err(_) => false,
    }
}

/// Fusiona los envs del override sobre `base` (los del .env / pantalla Envs). En
/// conflicto de clave **gana el override** (es el arreglo deliberado del agente).
pub fn merge_envs(base: Vec<(String, String)>, ovr: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = base;
    for (k, v) in ovr {
        if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == k) {
            slot.1 = v.clone();
        } else {
            out.push((k.clone(), v.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn override_wins_on_conflict_and_appends_new() {
        let base = vec![e("PORT", "3000"), e("NODE_ENV", "dev")];
        let ovr = vec![e("NODE_ENV", "production"), e("DATABASE_URL", "mongo://x")];
        let merged = merge_envs(base, &ovr);
        // NODE_ENV lo gana el override (arreglo del agente).
        assert_eq!(merged.iter().find(|(k, _)| k == "NODE_ENV").unwrap().1, "production");
        // PORT base intacto.
        assert_eq!(merged.iter().find(|(k, _)| k == "PORT").unwrap().1, "3000");
        // La clave nueva del agente se agrega.
        assert!(merged.iter().any(|(k, _)| k == "DATABASE_URL"));
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn empty_override_is_identity() {
        let base = vec![e("A", "1")];
        assert_eq!(merge_envs(base.clone(), &[]), base);
    }
}
