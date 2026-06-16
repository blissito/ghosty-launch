//! Cliente HTTP delgado para la API REST v2 de EasyBits.
//! Solo los endpoints que el camino feliz del launcher necesita.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://www.easybits.cloud";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Sandbox {
    #[serde(rename = "sandboxId", alias = "id")]
    pub sandbox_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SandboxList {
    #[serde(default)]
    sandboxes: Option<Vec<Sandbox>>,
}

/// Respuesta de POST /sandboxes/:id/bg — arranca un comando en background.
#[derive(Debug, Deserialize)]
pub struct BgStart {
    #[serde(rename = "execId", alias = "exec_id")]
    pub exec_id: String,
}

/// Respuesta de GET /sandboxes/:id/bg/:execId — estado + logs capturados.
#[derive(Debug, Deserialize)]
pub struct BgStatus {
    /// "running" | "exited"
    pub status: String,
    #[serde(rename = "exitCode", alias = "exit_code", default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

#[derive(Debug, Deserialize)]
pub struct Exposed {
    pub url: String,
}

impl Client {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let base_url =
            std::env::var("EASYBITS_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
        let http = reqwest::Client::builder()
            .user_agent(concat!("ghosty-launch/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v2{}", self.base_url, path)
    }

    /// GET /api/v2/me — valida la llave y devuelve el dueño.
    pub async fn validate(&self) -> Result<Me> {
        let resp = self
            .http
            .get(self.url("/me"))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("Llave inválida (401). Revisa tu eb_sk_…"));
        }
        ensure_ok(&resp.status(), "GET /me")?;
        Ok(resp.json::<Me>().await?)
    }

    /// POST /api/v2/sandboxes — crea una VM (persistente para hostear).
    /// `name` la marca para poder reencontrarla en runs posteriores.
    /// `size` = clase de tamaño (s/m/l/xl); EasyBits la mapea a CPU/RAM/disco.
    pub async fn create_sandbox(
        &self,
        template: &str,
        persistent: bool,
        name: &str,
        size: &str,
    ) -> Result<Sandbox> {
        let resp = self
            .http
            .post(self.url("/sandboxes"))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "template": template,
                "persistent": persistent,
                "name": name,
                "size": size,
            }))
            .send()
            .await?;
        let resp = ensure_ok_body(resp, "POST /sandboxes").await?;
        Ok(resp.json::<Sandbox>().await?)
    }

    /// GET /api/v2/sandboxes — lista las VMs del dueño.
    pub async fn list_sandboxes(&self) -> Result<Vec<Sandbox>> {
        let resp = self
            .http
            .get(self.url("/sandboxes"))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        ensure_ok(&resp.status(), "GET /sandboxes")?;
        Ok(resp
            .json::<SandboxList>()
            .await?
            .sandboxes
            .unwrap_or_default())
    }

    /// GET /api/v2/sandboxes/:id — estado de la VM (para poll hasta running).
    pub async fn get_sandbox(&self, id: &str) -> Result<Sandbox> {
        let resp = self
            .http
            .get(self.url(&format!("/sandboxes/{id}")))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        ensure_ok(&resp.status(), "GET /sandboxes/:id")?;
        Ok(resp.json::<Sandbox>().await?)
    }

    /// POST /api/v2/sandboxes/:id/bg — arranca un comando en background y vuelve
    /// al instante con un execId. Para deploys largos (install+build): la petición
    /// no queda abierta, así que un build que satura la micro-VM no tira la conexión
    /// (patrón resiliente tipo E2B `background:true` + poll).
    pub async fn exec_background(&self, id: &str, command: &str) -> Result<BgStart> {
        let resp = self
            .http
            .post(self.url(&format!("/sandboxes/{id}/bg")))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "command": command }))
            .send()
            .await?;
        let resp = ensure_ok_body(resp, "POST /sandboxes/:id/bg").await?;
        Ok(resp.json::<BgStart>().await?)
    }

    /// GET /api/v2/sandboxes/:id/bg/:execId — estado + logs del proceso background.
    pub async fn exec_status(&self, id: &str, exec_id: &str) -> Result<BgStatus> {
        let resp = self
            .http
            .get(self.url(&format!("/sandboxes/{id}/bg/{exec_id}")))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let resp = ensure_ok_body(resp, "GET /sandboxes/:id/bg/:execId").await?;
        Ok(resp.json::<BgStatus>().await?)
    }

    /// POST /api/v2/sandboxes/:id/expose — publica un puerto y devuelve la URL.
    pub async fn expose(&self, id: &str, port: u16) -> Result<Exposed> {
        let resp = self
            .http
            .post(self.url(&format!("/sandboxes/{id}/expose")))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "port": port }))
            .send()
            .await?;
        ensure_ok(&resp.status(), "POST /sandboxes/:id/expose")?;
        Ok(resp.json::<Exposed>().await?)
    }

    /// Sube un archivo PÚBLICO y devuelve su URL embebible.
    /// Flujo EasyBits: POST /files (access:public) → {file.url, putUrl} → PUT bytes.
    pub async fn upload_public_file(
        &self,
        file_name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String> {
        let resp = self
            .http
            .post(self.url("/files"))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "fileName": file_name,
                "contentType": content_type,
                "size": bytes.len(),
                "access": "public",
            }))
            .send()
            .await?;
        ensure_ok(&resp.status(), "POST /files")?;
        let v: serde_json::Value = resp.json().await?;
        let put_url = v
            .get("putUrl")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("respuesta sin putUrl"))?
            .to_string();
        let url = v
            .get("file")
            .and_then(|f| f.get("url"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("respuesta sin url pública"))?
            .to_string();

        let put = self
            .http
            .put(&put_url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await?;
        if !put.status().is_success() {
            return Err(anyhow!(
                "PUT del logo falló: HTTP {}",
                put.status().as_u16()
            ));
        }
        Ok(url)
    }

    /// POST /api/v2/websites — crea un sitio estático (CDN). Devuelve (id, url pública).
    pub async fn create_website(&self, name: &str) -> Result<(String, String)> {
        let resp = self
            .http
            .post(self.url("/websites"))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await?;
        ensure_ok(&resp.status(), "POST /websites")?;
        let v: serde_json::Value = resp.json().await?;
        let w = v.get("website").unwrap_or(&v);
        let id = w
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("respuesta sin website.id"))?
            .to_string();
        let url = w
            .get("url")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        Ok((id, url))
    }

    /// Sube un archivo estático a un website (presigned PUT). `path` puede llevar subdirs.
    pub async fn upload_website_file(
        &self,
        website_id: &str,
        path: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let resp = self
            .http
            .post(self.url(&format!("/websites/{website_id}/files")))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "fileName": path,
                "contentType": content_type,
                "size": bytes.len(),
            }))
            .send()
            .await?;
        ensure_ok(&resp.status(), "POST /websites/:id/files")?;
        let v: serde_json::Value = resp.json().await?;
        let put_url = v
            .get("putUrl")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("respuesta sin putUrl"))?
            .to_string();
        let put = self
            .http
            .put(&put_url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await?;
        if !put.status().is_success() {
            return Err(anyhow!("PUT {path} falló: HTTP {}", put.status().as_u16()));
        }
        Ok(())
    }

    /// GET crudo (status + body sin parsear) para debug.
    pub async fn get_raw(&self, path: &str) -> Result<(u16, String)> {
        let resp = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        Ok((status, body))
    }

    /// POST crudo (status + body sin parsear) para debug.
    pub async fn post_raw(&self, path: &str, json: serde_json::Value) -> Result<(u16, String)> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .json(&json)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        Ok((status, body))
    }

    /// DELETE /api/v2/sandboxes/:id — destruye la VM (cleanup).
    pub async fn destroy(&self, id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/sandboxes/{id}")))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        ensure_ok(&resp.status(), "DELETE /sandboxes/:id")?;
        Ok(())
    }
}

fn ensure_ok(status: &reqwest::StatusCode, what: &str) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(anyhow!("{what} falló: HTTP {}", status.as_u16()))
    }
}

/// Como `ensure_ok` pero consume la respuesta y adjunta el cuerpo del error
/// (recortado) — útil para ver el motivo real de un 500 del host.
async fn ensure_ok_body(resp: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let snippet: String = body.trim().chars().take(400).collect();
    if snippet.is_empty() {
        Err(anyhow!("{what} falló: HTTP {}", status.as_u16()))
    } else {
        Err(anyhow!("{what} falló: HTTP {} — {snippet}", status.as_u16()))
    }
}
