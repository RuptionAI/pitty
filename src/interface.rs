//! Where a chat turn is sent. The review in front of the turn does not change.
use clap::ValueEnum;

pub const OPENROUTER_ROOT: &str = "https://openrouter.ai/api/v1";
pub const OPENAI_ROOT: &str = "https://api.openai.com/v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum InterfaceKind {
    /// OpenRouter chat completions, including its catalog, web search, and PDFs.
    #[default]
    OpenRouter,
    /// OpenAI chat completions. The caller supplies the model id.
    OpenAI,
    /// Another chat endpoint that speaks the same protocol. Requires a base URL.
    Compatible,
}

impl InterfaceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::OpenAI => "openai",
            Self::Compatible => "compatible",
        }
    }
    pub fn error_label(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::OpenAI => "OpenAI",
            Self::Compatible => "Chat endpoint",
        }
    }
    pub fn key_var(self) -> &'static str {
        match self {
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::Compatible => "PITTY_API_KEY",
        }
    }
    /// OpenRouter request fields: plugins, hosted web search, and the `:online` suffix.
    pub fn extensions(self) -> bool {
        matches!(self, Self::OpenRouter)
    }
    /// A local or private endpoint may accept a request with no credential.
    pub fn allows_missing_key(self) -> bool {
        matches!(self, Self::Compatible)
    }
}

pub fn chat_endpoint(kind: InterfaceKind, base_url: Option<&str>) -> Result<String, String> {
    Ok(format!("{}/chat/completions", root(kind, base_url)?))
}

pub fn models_endpoint(kind: InterfaceKind, base_url: Option<&str>) -> Result<String, String> {
    let root = root(kind, base_url)?;
    if kind.extensions() {
        Ok(format!("{root}/models?output_modalities=all"))
    } else {
        Ok(format!("{root}/models"))
    }
}

pub fn check_model_id(kind: InterfaceKind, id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 200 || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("A model id is one token, at most 200 characters.".into());
    }
    if kind.extensions() && !id.contains('/') {
        return Err(format!(
            "Use an OpenRouter model id such as {}.",
            pitty_core::MODEL
        ));
    }
    Ok(())
}

fn root(kind: InterfaceKind, base_url: Option<&str>) -> Result<String, String> {
    let supplied = base_url.map(str::trim).filter(|url| !url.is_empty());
    match (kind, supplied) {
        (InterfaceKind::OpenRouter, None) => Ok(OPENROUTER_ROOT.into()),
        (InterfaceKind::OpenAI, None) => Ok(OPENAI_ROOT.into()),
        (InterfaceKind::Compatible, Some(url)) => normalize_root(url),
        (InterfaceKind::Compatible, None) => Err(
            "The compatible interface needs --base-url, for example http://127.0.0.1:11434/v1."
                .into(),
        ),
        (_, Some(_)) => {
            Err("--base-url belongs to --interface compatible. OpenRouter and OpenAI have fixed endpoints.".into())
        }
    }
}

fn normalize_root(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    if url.len() > 2000
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains('@')
        || url.contains('?')
        || url.contains('#')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(
            "Base URL must be an http:// or https:// address, without credentials, spaces, or a query."
                .into(),
        );
    }
    Ok(url
        .strip_suffix("/chat/completions")
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_interface_has_one_chat_endpoint() {
        assert_eq!(
            chat_endpoint(InterfaceKind::OpenRouter, None).unwrap(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            chat_endpoint(InterfaceKind::OpenAI, None).unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            chat_endpoint(
                InterfaceKind::Compatible,
                Some("http://127.0.0.1:11434/v1/"),
            )
            .unwrap(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_endpoint(
                InterfaceKind::Compatible,
                Some("http://127.0.0.1:8080/v1/chat/completions"),
            )
            .unwrap(),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert!(chat_endpoint(InterfaceKind::Compatible, None).is_err());
        assert!(chat_endpoint(InterfaceKind::OpenAI, Some("https://example.com/v1")).is_err());
        assert!(normalize_root("https://user:secret@example.com/v1").is_err());
        assert!(normalize_root("http://127.0.0.1:11434/v1?token=secret").is_err());
        assert!(check_model_id(InterfaceKind::OpenRouter, "gpt-4.1-mini").is_err());
        assert!(check_model_id(InterfaceKind::OpenAI, "gpt-4.1-mini").is_ok());
    }
}
