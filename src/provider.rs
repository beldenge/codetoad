#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Xai,
    OpenAi,
    Compatible,
}

pub const XAI_DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
pub const XAI_DEFAULT_MODEL: &str = "grok-code-fast-1";

pub fn detect_provider(base_url: &str) -> ProviderKind {
    if let Some(host) = extract_host(base_url) {
        if host == "api.x.ai" || host.ends_with(".x.ai") {
            return ProviderKind::Xai;
        }
        if host == "api.openai.com" || host.ends_with(".openai.com") {
            return ProviderKind::OpenAi;
        }
    }

    let lowered = base_url.trim().to_ascii_lowercase();
    if lowered.contains("api.x.ai") {
        return ProviderKind::Xai;
    }
    if lowered.contains("api.openai.com") {
        return ProviderKind::OpenAi;
    }
    ProviderKind::Compatible
}

pub fn default_model_for(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Xai => "grok-code-fast-1",
        ProviderKind::OpenAi => "gpt-4.1",
        ProviderKind::Compatible => "gpt-4.1-mini",
    }
}

pub fn default_models_for(provider: ProviderKind) -> Vec<String> {
    match provider {
        ProviderKind::Xai => vec![
            "grok-4-1-fast-reasoning".to_string(),
            "grok-4-1-fast-non-reasoning".to_string(),
            "grok-4-fast-reasoning".to_string(),
            "grok-4-fast-non-reasoning".to_string(),
            "grok-4".to_string(),
            "grok-4-latest".to_string(),
            "grok-code-fast-1".to_string(),
            "grok-3".to_string(),
            "grok-3-latest".to_string(),
            "grok-3-fast".to_string(),
            "grok-3-mini".to_string(),
            "grok-3-mini-fast".to_string(),
        ],
        ProviderKind::OpenAi | ProviderKind::Compatible => vec![
            "gpt-4.1".to_string(),
            "gpt-4.1-mini".to_string(),
            "gpt-4o".to_string(),
            "gpt-4o-mini".to_string(),
            "o3".to_string(),
            "o4-mini".to_string(),
        ],
    }
}

pub fn api_key_env_candidates(provider: ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::Xai => &["GROK_API_KEY", "XAI_API_KEY", "OPENAI_API_KEY"],
        ProviderKind::OpenAi => &["GROK_API_KEY", "OPENAI_API_KEY", "XAI_API_KEY"],
        ProviderKind::Compatible => &["GROK_API_KEY", "OPENAI_API_KEY", "XAI_API_KEY"],
    }
}

fn extract_host(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let no_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = no_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        return None;
    }
    let without_auth = authority.rsplit('@').next().unwrap_or(authority).trim();
    let host = without_auth
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

#[cfg(test)]
mod tests {
    use super::{ProviderKind, detect_provider};

    #[test]
    fn detects_xai_from_host() {
        assert_eq!(detect_provider("https://api.x.ai/v1"), ProviderKind::Xai);
        assert_eq!(
            detect_provider("https://proxy.api.x.ai/custom"),
            ProviderKind::Xai
        );
    }

    #[test]
    fn detects_openai_from_host() {
        assert_eq!(
            detect_provider("https://api.openai.com/v1"),
            ProviderKind::OpenAi
        );
    }

    #[test]
    fn falls_back_to_compatible_for_unknown_hosts() {
        assert_eq!(
            detect_provider("https://example.com/v1"),
            ProviderKind::Compatible
        );
    }
}
