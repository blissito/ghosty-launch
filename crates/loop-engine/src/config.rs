//! Minimal config surface extracted from `ghostycode/crates/tui/src/config.rs`
//! (10306 lines in the original; we bring only what the vendored client needs).
//!
//! Extracted verbatim:
//!   - `ApiProvider` enum + `parse` / `as_str` / `display_name` / `all`
//!   - `RetryPolicy` struct
//!   - `provider_passes_model_through`
//!   - `canonical_official_deepseek_model_id`
//!
//! Trimmed (annotated below):
//!   - `wire_model_for_provider` — the original delegates to a large
//!     model-name normalization tree (`normalize_model_name_for_provider` and
//!     ~50 provider model-id tables/constants). The vendored engine only ever
//!     wires already-canonical DeepSeek model IDs, so we keep the exact
//!     pass-through + official-DeepSeek-canonicalization behavior and drop the
//!     per-provider catalog rewriting. This is NOT reasoning logic.
//!   - `Config` / `ProviderConfig` — reduced to the fields/methods that
//!     `DeepSeekClient::new` reads. The original loads TOML, keyring, env
//!     overrides, etc. None of that is exercised by the reasoning corpus; the
//!     engine is driven directly via `MessageRequest`. Construct with
//!     `Config::for_endpoint(..)` when you do want a live client.

use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

// === ApiProvider (verbatim) ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    Deepseek,
    DeepseekCN,
    NvidiaNim,
    Openai,
    Atlascloud,
    WanjieArk,
    Volcengine,
    Openrouter,
    XiaomiMimo,
    Novita,
    Fireworks,
    Siliconflow,
    SiliconflowCn,
    Arcee,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    Huggingface,
}

impl ApiProvider {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "deepseek" | "deep-seek" => Some(Self::Deepseek),
            "easybits" | "easy-bits" | "easy_bits" | "eb" => Some(Self::Deepseek),
            "deepseek-cn" | "deepseek_china" | "deepseekcn" | "deepseek-china" => {
                Some(Self::DeepseekCN)
            }
            "nvidia" | "nvidia-nim" | "nvidia_nim" | "nim" => Some(Self::NvidiaNim),
            "openai" | "open-ai" => Some(Self::Openai),
            "atlascloud" | "atlas-cloud" | "atlas_cloud" | "atlas" => Some(Self::Atlascloud),
            "wanjie" | "wanjie-ark" | "wanjie_ark" | "ark-wanjie" | "ark_wanjie" | "wanjieark"
            | "wanjie-maas" | "wanjie_maas" | "wanjiemaas" => Some(Self::WanjieArk),
            "volcengine" | "volcengine-ark" | "volcengine_ark" | "ark" | "volc-ark"
            | "volcengineark" => Some(Self::Volcengine),
            "openrouter" | "open_router" => Some(Self::Openrouter),
            "xiaomi-mimo" | "xiaomi_mimo" | "xiaomimimo" | "mimo" | "xiaomi" => {
                Some(Self::XiaomiMimo)
            }
            "novita" => Some(Self::Novita),
            "fireworks" | "fireworks-ai" => Some(Self::Fireworks),
            "siliconflow" | "silicon-flow" | "silicon_flow" => Some(Self::Siliconflow),
            "siliconflow-cn" | "siliconflow-CN" | "silicon-flow-cn" | "silicon-flow-CN"
            | "silicon_flow_cn" | "silicon_flow_CN" | "siliconflow-china" => {
                Some(Self::SiliconflowCn)
            }
            "arcee" | "arcee-ai" | "arcee_ai" => Some(Self::Arcee),
            "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => Some(Self::Moonshot),
            "sglang" | "sg-lang" => Some(Self::Sglang),
            "vllm" | "v-llm" => Some(Self::Vllm),
            "ollama" | "ollama-local" => Some(Self::Ollama),
            "huggingface" | "hugging-face" | "hugging_face" | "hf" => Some(Self::Huggingface),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::DeepseekCN => "deepseek-cn",
            Self::NvidiaNim => "nvidia-nim",
            Self::Openai => "openai",
            Self::Atlascloud => "atlascloud",
            Self::WanjieArk => "wanjie-ark",
            Self::Volcengine => "volcengine",
            Self::Openrouter => "openrouter",
            Self::XiaomiMimo => "xiaomi-mimo",
            Self::Novita => "novita",
            Self::Fireworks => "fireworks",
            Self::Siliconflow => "siliconflow",
            Self::SiliconflowCn => "siliconflow-CN",
            Self::Arcee => "arcee",
            Self::Moonshot => "moonshot",
            Self::Sglang => "sglang",
            Self::Vllm => "vllm",
            Self::Ollama => "ollama",
            Self::Huggingface => "huggingface",
        }
    }

    /// Human-friendly label for picker UIs / status chips.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Deepseek => "DeepSeek",
            Self::DeepseekCN => "DeepSeek (legacy alias)",
            Self::NvidiaNim => "NVIDIA NIM",
            Self::Openai => "OpenAI-compatible",
            Self::Atlascloud => "AtlasCloud",
            Self::WanjieArk => "Wanjie Ark",
            Self::Volcengine => "Volcengine Ark",
            Self::Openrouter => "OpenRouter",
            Self::XiaomiMimo => "Xiaomi MiMo",
            Self::Novita => "Novita AI",
            Self::Fireworks => "Fireworks AI",
            Self::Siliconflow => "SiliconFlow",
            Self::SiliconflowCn => "SiliconFlow (China)",
            Self::Arcee => "Arcee AI",
            Self::Moonshot => "Moonshot/Kimi",
            Self::Sglang => "SGLang",
            Self::Vllm => "vLLM",
            Self::Ollama => "Ollama",
            Self::Huggingface => "Hugging Face",
        }
    }

    /// All providers, in declaration order.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Deepseek,
            Self::DeepseekCN,
            Self::NvidiaNim,
            Self::Openai,
            Self::Atlascloud,
            Self::WanjieArk,
            Self::Volcengine,
            Self::Openrouter,
            Self::XiaomiMimo,
            Self::Novita,
            Self::Fireworks,
            Self::Siliconflow,
            Self::SiliconflowCn,
            Self::Arcee,
            Self::Moonshot,
            Self::Sglang,
            Self::Vllm,
            Self::Ollama,
            Self::Huggingface,
        ]
    }
}

// === Model wiring ===

/// Providers that send the model name through unchanged (verbatim).
pub(crate) fn provider_passes_model_through(provider: ApiProvider) -> bool {
    matches!(
        provider,
        ApiProvider::Openai
            | ApiProvider::Atlascloud
            | ApiProvider::WanjieArk
            | ApiProvider::Volcengine
            | ApiProvider::XiaomiMimo
            | ApiProvider::Moonshot
            | ApiProvider::Ollama
            | ApiProvider::Huggingface
    )
}

/// Canonicalize the official DeepSeek model ids (verbatim).
fn canonical_official_deepseek_model_id(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        "deepseek-v4-pro"
        | "deepseek-v4pro"
        | "deepseek-ai/deepseek-v4-pro"
        | "deepseek-ai/deepseek-v4pro"
        | "deepseek/deepseek-v4-pro"
        | "deepseek/deepseek-v4pro" => Some("deepseek-v4-pro"),
        "deepseek-v4-flash"
        | "deepseek-v4flash"
        | "deepseek-ai/deepseek-v4-flash"
        | "deepseek-ai/deepseek-v4flash"
        | "deepseek/deepseek-v4-flash"
        | "deepseek/deepseek-v4flash" => Some("deepseek-v4-flash"),
        _ => None,
    }
}

/// Resolve the wire model name for a provider.
///
/// TRIMMED from ghostycode's `wire_model_for_provider`: the original routes
/// through `normalize_model_name_for_provider`, which carries per-provider
/// model catalogs (OpenRouter/Xiaomi/Arcee/SiliconFlow id rewriting) totalling
/// ~50 constants and a dozen helpers — none of which the vendored engine
/// exercises. We preserve the two behaviors the engine actually depends on:
///   1. pass-through providers send the (trimmed) name unchanged;
///   2. DeepSeek-family providers canonicalize the official V4 ids
///      (`deepseek-ai/DeepSeek-V4-Pro` → `deepseek-v4-pro`, etc.).
/// Any other name is sent through trimmed-but-unchanged, which matches the
/// original's fallback (`unwrap_or_else(|| trimmed.to_string())`).
#[must_use]
pub fn wire_model_for_provider(provider: ApiProvider, model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    if provider_passes_model_through(provider) {
        return trimmed.to_string();
    }
    if let Some(canonical) = canonical_official_deepseek_model_id(trimmed) {
        // Preserve the user's casing when it already matches case-insensitively;
        // only rewrite compact/prefixed aliases. Mirrors the original.
        if canonical.eq_ignore_ascii_case(trimmed) {
            return trimmed.to_string();
        }
        return canonical.to_string();
    }
    trimmed.to_string()
}

// === RetryPolicy (verbatim) ===

/// Resolved retry policy with defaults applied.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub enabled: bool,
    pub max_retries: u32,
    pub initial_delay: f64,
    pub max_delay: f64,
    pub exponential_base: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 3,
            initial_delay: 1.0,
            max_delay: 60.0,
            exponential_base: 2.0,
        }
    }
}

// === Config (trimmed to what DeepSeekClient::new reads) ===

/// Per-provider connection overrides. Only the fields the client reads are
/// kept from ghostycode's much larger `ProviderConfig`.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    pub path_suffix: Option<String>,
}

/// Minimal client configuration. The full ghostycode `Config` loads TOML +
/// keyring + env; here we hold the resolved values directly so the vendored
/// `DeepSeekClient::new` compiles and a caller can build a live client.
#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub provider: ApiProvider,
    pub model: String,
    pub retry: RetryPolicy,
    pub headers: HashMap<String, String>,
    pub provider_config: ProviderConfig,
}

impl Config {
    /// Build a config for a live endpoint. The reasoning corpus never calls
    /// this — the engine is driven via `MessageRequest` — but the wiring
    /// helpers (`DeepSeekClient::new`) consume it.
    #[must_use]
    pub fn for_endpoint(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        provider: ApiProvider,
        model: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            provider,
            model: model.into(),
            retry: RetryPolicy::default(),
            headers: HashMap::new(),
            provider_config: ProviderConfig::default(),
        }
    }

    pub fn deepseek_api_key(&self) -> Result<String> {
        Ok(self.api_key.clone())
    }

    #[must_use]
    pub fn deepseek_base_url(&self) -> String {
        self.base_url.clone()
    }

    #[must_use]
    pub fn api_provider(&self) -> ApiProvider {
        self.provider
    }

    #[must_use]
    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry.clone()
    }

    #[must_use]
    pub fn default_model(&self) -> String {
        self.model.clone()
    }

    #[must_use]
    pub fn http_headers(&self) -> HashMap<String, String> {
        self.headers.clone()
    }

    #[must_use]
    pub fn provider_config_for(&self, provider: ApiProvider) -> Option<&ProviderConfig> {
        (provider == self.provider).then_some(&self.provider_config)
    }
}
