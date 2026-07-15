use std::{path::Path, time::Duration};

use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

pub const CLOUDFLARE_QUICK_ID: &str = "cloudflare-quick";

const INVALID_PROFILE: &str =
    "Porta couldn't save this provider. Check the highlighted fields, then try again.";
const MISSING_EXECUTABLE: &str = "Choose the provider's executable file, then try again.";
const INVALID_EXECUTABLE: &str =
    "Porta can't use that executable. Choose the provider's executable file again.";
const INVALID_PUBLIC_URL: &str = "Enter the provider's complete public HTTPS URL, then try again.";
const INVALID_PATTERN: &str =
    "Enter a valid output pattern that finds the provider's public URL or ready message.";
const INVALID_ENVIRONMENT_NAME: &str =
    "Enter an environment variable name using letters, numbers, and underscores.";
const INVALID_HEADER: &str =
    "Enter one HTTP header name for visitor addresses, such as X-Forwarded-For.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    CloudflareQuick,
    CloudflareManaged,
    Ngrok,
    Custom,
}

impl ProviderKind {
    pub const fn requires_credential(self, credential_env: Option<&str>) -> bool {
        match self {
            Self::CloudflareManaged | Self::Ngrok => true,
            Self::Custom => credential_env.is_some(),
            Self::CloudflareQuick => false,
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CloudflareQuick => "Cloudflare Quick Tunnel",
            Self::CloudflareManaged => "Cloudflare managed tunnel",
            Self::Ngrok => "ngrok",
            Self::Custom => "Custom command",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_ip_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfileView {
    #[serde(flatten)]
    pub profile: ProviderProfile,
    pub built_in: bool,
    pub credential_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderProfileInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_ip_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    #[serde(default)]
    pub clear_credential: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResult {
    pub url: String,
    pub message: String,
}

#[derive(Clone)]
pub(crate) enum ProviderProgram {
    BundledCloudflared,
    External(String),
}

#[derive(Clone)]
pub(crate) enum UrlDiscovery {
    Output {
        pattern: Regex,
        fixed_url: Option<String>,
    },
    FixedAfter {
        url: String,
        delay: Duration,
    },
}

impl UrlDiscovery {
    pub fn inspect(&self, output: &str) -> Option<String> {
        let Self::Output { pattern, fixed_url } = self else {
            return None;
        };
        let captures = pattern.captures(output)?;
        let candidate = captures
            .name("url")
            .or_else(|| captures.get(0))
            .map(|value| value.as_str());
        candidate
            .filter(|value| is_public_https_url(value))
            .map(str::to_owned)
            .or_else(|| fixed_url.clone())
    }

    pub const fn fixed_delay(&self) -> Option<Duration> {
        match self {
            Self::FixedAfter { delay, .. } => Some(*delay),
            Self::Output { .. } => None,
        }
    }

    pub fn fixed_url(&self) -> Option<String> {
        match self {
            Self::FixedAfter { url, .. } => Some(url.clone()),
            Self::Output { .. } => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProviderLaunch {
    pub program: ProviderProgram,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub discovery: UrlDiscovery,
    pub start_error: String,
    pub connection_error: String,
}

#[derive(Clone)]
pub(crate) struct ResolvedProvider {
    pub profile: ProviderProfile,
    credential: Option<String>,
}

impl ResolvedProvider {
    pub fn new(profile: ProviderProfile, credential: Option<String>) -> Result<Self, String> {
        validate_profile(&profile)?;
        if profile
            .kind
            .requires_credential(profile.credential_env.as_deref())
            && credential.as_deref().is_none_or(str::is_empty)
        {
            return Err(format!(
                "Add the credential for “{}” in Settings, then try again.",
                profile.name
            ));
        }
        Ok(Self {
            profile,
            credential,
        })
    }

    pub const fn preferred_local_port(&self) -> Option<u16> {
        self.profile.local_port
    }

    pub fn visitor_headers(&self) -> Vec<String> {
        let mut headers = Vec::new();
        if let Some(header) = self.profile.forwarded_ip_header.as_deref() {
            headers.push(header.to_ascii_lowercase());
        }
        match self.profile.kind {
            ProviderKind::CloudflareQuick | ProviderKind::CloudflareManaged => {
                append_unique(&mut headers, "cf-connecting-ip");
                append_unique(&mut headers, "x-forwarded-for");
                append_unique(&mut headers, "x-real-ip");
            }
            ProviderKind::Ngrok | ProviderKind::Custom => {
                append_unique(&mut headers, "x-forwarded-for");
                append_unique(&mut headers, "x-real-ip");
                append_unique(&mut headers, "cf-connecting-ip");
            }
        }
        headers
    }

    pub fn launch(&self, origin: std::net::SocketAddr) -> Result<ProviderLaunch, String> {
        let origin_url = format!("http://{origin}");
        match self.profile.kind {
            ProviderKind::CloudflareQuick => Ok(ProviderLaunch {
                program: ProviderProgram::BundledCloudflared,
                arguments: vec![
                    "tunnel".to_owned(),
                    "--url".to_owned(),
                    origin_url,
                    "--no-autoupdate".to_owned(),
                ],
                environment: Vec::new(),
                discovery: UrlDiscovery::Output {
                    pattern: Regex::new(
                        r"(?P<url>https://[a-z0-9-]+\.trycloudflare\.com)",
                    )
                    .expect("Cloudflare Quick URL pattern should compile"),
                    fixed_url: None,
                },
                start_error: "Porta couldn't start Cloudflare's tunnel helper. Quit and reopen Porta, then try again.".to_owned(),
                connection_error: "Couldn't reach Cloudflare — check your internet connection and try again.".to_owned(),
            }),
            ProviderKind::CloudflareManaged => {
                let public_url = self.profile.public_url.clone().ok_or_else(|| {
                    "Add this Cloudflare tunnel's public URL in Settings, then try again."
                        .to_owned()
                })?;
                Ok(ProviderLaunch {
                    program: ProviderProgram::BundledCloudflared,
                    arguments: vec![
                        "tunnel".to_owned(),
                        "--no-autoupdate".to_owned(),
                        "run".to_owned(),
                    ],
                    environment: vec![(
                        "TUNNEL_TOKEN".to_owned(),
                        self.credential.clone().unwrap_or_default(),
                    )],
                    discovery: UrlDiscovery::Output {
                        pattern: Regex::new(r"(?i)registered tunnel connection")
                            .expect("Cloudflare readiness pattern should compile"),
                        fixed_url: Some(public_url),
                    },
                    start_error: "Porta couldn't start this managed Cloudflare tunnel. Check its profile, then try again.".to_owned(),
                    connection_error: "Porta couldn't connect this managed Cloudflare tunnel. Check its token and dashboard route, then try again.".to_owned(),
                })
            }
            ProviderKind::Ngrok => {
                let executable = self.profile.executable.clone().ok_or_else(|| {
                    "Choose the ngrok executable in Settings, then try again.".to_owned()
                })?;
                let mut arguments = vec![
                    "http".to_owned(),
                    origin_url,
                    "--log".to_owned(),
                    "stdout".to_owned(),
                    "--log-format".to_owned(),
                    "json".to_owned(),
                ];
                if let Some(public_url) = self.profile.public_url.as_deref() {
                    arguments.push("--url".to_owned());
                    arguments.push(public_url.to_owned());
                }
                Ok(ProviderLaunch {
                    program: ProviderProgram::External(executable),
                    arguments,
                    environment: vec![(
                        "NGROK_AUTHTOKEN".to_owned(),
                        self.credential.clone().unwrap_or_default(),
                    )],
                    discovery: UrlDiscovery::Output {
                        pattern: Regex::new(
                            r"(?P<url>https://[A-Za-z0-9._-]+\.(?:ngrok-free\.app|ngrok\.app|ngrok\.dev|ngrok\.io))",
                        )
                        .expect("ngrok URL pattern should compile"),
                        fixed_url: self.profile.public_url.clone(),
                    },
                    start_error: "Porta couldn't start ngrok. Check its executable in Settings, then try again.".to_owned(),
                    connection_error: "Porta couldn't connect through ngrok. Check its token and internet connection, then try again.".to_owned(),
                })
            }
            ProviderKind::Custom => {
                let executable = self.profile.executable.clone().ok_or_else(|| {
                    "Choose this provider's executable in Settings, then try again.".to_owned()
                })?;
                let arguments = render_arguments(&self.profile.arguments, origin);
                let environment = match (
                    self.profile.credential_env.as_deref(),
                    self.credential.as_deref(),
                ) {
                    (Some(name), Some(value)) => vec![(name.to_owned(), value.to_owned())],
                    _ => Vec::new(),
                };
                let discovery = match (
                    self.profile.url_pattern.as_deref(),
                    self.profile.public_url.as_deref(),
                ) {
                    (Some(pattern), fixed_url) => UrlDiscovery::Output {
                        pattern: Regex::new(pattern).map_err(|_| INVALID_PATTERN.to_owned())?,
                        fixed_url: fixed_url.map(str::to_owned),
                    },
                    (None, Some(url)) => UrlDiscovery::FixedAfter {
                        url: url.to_owned(),
                        delay: Duration::from_secs(1),
                    },
                    (None, None) => return Err(INVALID_PATTERN.to_owned()),
                };
                Ok(ProviderLaunch {
                    program: ProviderProgram::External(executable),
                    arguments,
                    environment,
                    discovery,
                    start_error: format!(
                        "Porta couldn't start “{}”. Check its executable and arguments, then try again.",
                        self.profile.name
                    ),
                    connection_error: format!(
                        "Porta couldn't get a public link from “{}”. Check its output pattern, then try again.",
                        self.profile.name
                    ),
                })
            }
        }
    }
}

pub fn cloudflare_quick_profile() -> ProviderProfile {
    ProviderProfile {
        id: CLOUDFLARE_QUICK_ID.to_owned(),
        name: ProviderKind::CloudflareQuick.display_name().to_owned(),
        kind: ProviderKind::CloudflareQuick,
        executable: None,
        arguments: Vec::new(),
        public_url: None,
        url_pattern: None,
        credential_env: None,
        forwarded_ip_header: Some("CF-Connecting-IP".to_owned()),
        local_port: None,
        created_at: "built-in".to_owned(),
    }
}

pub fn profile_by_id(profiles: &[ProviderProfile], id: &str) -> Option<ProviderProfile> {
    if id == CLOUDFLARE_QUICK_ID {
        Some(cloudflare_quick_profile())
    } else {
        profiles.iter().find(|profile| profile.id == id).cloned()
    }
}

pub fn profile_view(profile: ProviderProfile, credential_configured: bool) -> ProviderProfileView {
    let built_in = profile.id == CLOUDFLARE_QUICK_ID;
    ProviderProfileView {
        profile,
        built_in,
        credential_configured,
    }
}

pub fn build_profile(
    input: SaveProviderProfileInput,
    existing: Option<&ProviderProfile>,
) -> Result<ProviderProfile, String> {
    if input.kind == ProviderKind::CloudflareQuick {
        return Err("Cloudflare Quick Tunnel is built in and doesn't need a profile.".to_owned());
    }

    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("Give this provider a name up to 64 characters, then try again.".to_owned());
    }
    let id = existing
        .map(|profile| profile.id.clone())
        .or(input.id)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if id == CLOUDFLARE_QUICK_ID || id.trim().is_empty() {
        return Err(INVALID_PROFILE.to_owned());
    }

    let executable = normalized_optional(input.executable);
    let public_url = normalized_optional(input.public_url);
    let url_pattern = normalized_optional(input.url_pattern);
    let credential_env = normalized_optional(input.credential_env);
    let forwarded_ip_header = normalized_optional(input.forwarded_ip_header);
    let (
        executable,
        arguments,
        public_url,
        url_pattern,
        credential_env,
        forwarded_ip_header,
        local_port,
    ) = match input.kind {
        ProviderKind::CloudflareManaged => (
            None,
            Vec::new(),
            public_url,
            None,
            None,
            None,
            input.local_port,
        ),
        ProviderKind::Ngrok => (executable, Vec::new(), public_url, None, None, None, None),
        ProviderKind::Custom => (
            executable,
            input.arguments,
            public_url,
            url_pattern,
            credential_env,
            forwarded_ip_header,
            input.local_port,
        ),
        ProviderKind::CloudflareQuick => unreachable!("built-in profile returned above"),
    };

    let profile = ProviderProfile {
        id,
        name: name.to_owned(),
        kind: input.kind,
        executable,
        arguments,
        public_url,
        url_pattern,
        credential_env,
        forwarded_ip_header,
        local_port,
        created_at: existing
            .map(|profile| profile.created_at.clone())
            .unwrap_or_else(|| Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
    };
    validate_profile(&profile)?;
    Ok(profile)
}

pub fn validate_profile(profile: &ProviderProfile) -> Result<(), String> {
    match profile.kind {
        ProviderKind::CloudflareQuick => {
            if profile.id != CLOUDFLARE_QUICK_ID {
                return Err(INVALID_PROFILE.to_owned());
            }
        }
        ProviderKind::CloudflareManaged => {
            validate_public_url(profile.public_url.as_deref())?;
            if profile.local_port.is_none_or(|port| port == 0) {
                return Err(
                    "Enter the local port used by this Cloudflare dashboard route, then try again."
                        .to_owned(),
                );
            }
        }
        ProviderKind::Ngrok => {
            validate_executable(profile.executable.as_deref())?;
            if profile.public_url.is_some() {
                validate_public_url(profile.public_url.as_deref())?;
            }
        }
        ProviderKind::Custom => {
            validate_executable(profile.executable.as_deref())?;
            if profile.public_url.is_some() {
                validate_public_url(profile.public_url.as_deref())?;
            }
            if let Some(pattern) = profile.url_pattern.as_deref() {
                if pattern.len() > 1024 || Regex::new(pattern).is_err() {
                    return Err(INVALID_PATTERN.to_owned());
                }
            } else if profile.public_url.is_none() {
                return Err(INVALID_PATTERN.to_owned());
            }
            if profile.arguments.len() > 64
                || profile
                    .arguments
                    .iter()
                    .any(|argument| argument.len() > 2048 || argument.contains('\0'))
            {
                return Err(
                    "Use at most 64 arguments of 2,048 characters each, then try again.".to_owned(),
                );
            }
            if profile.local_port == Some(0) {
                return Err("Enter a fixed local port from 1 to 65535, then try again.".to_owned());
            }
        }
    }

    if let Some(name) = profile.credential_env.as_deref() {
        let valid = name.bytes().enumerate().all(|(index, byte)| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
            b'0'..=b'9' => index > 0,
            _ => false,
        });
        if name.len() > 128 || !valid {
            return Err(INVALID_ENVIRONMENT_NAME.to_owned());
        }
    }
    if let Some(header) = profile.forwarded_ip_header.as_deref() {
        if header.len() > 128 || axum::http::HeaderName::from_bytes(header.as_bytes()).is_err() {
            return Err(INVALID_HEADER.to_owned());
        }
    }
    Ok(())
}

pub(crate) fn render_arguments(arguments: &[String], origin: std::net::SocketAddr) -> Vec<String> {
    let origin_url = format!("http://{origin}");
    arguments
        .iter()
        .map(|argument| {
            argument
                .replace("{origin}", &origin_url)
                .replace("{host}", &origin.ip().to_string())
                .replace("{port}", &origin.port().to_string())
        })
        .collect()
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn validate_executable(executable: Option<&str>) -> Result<(), String> {
    let executable = executable.ok_or_else(|| MISSING_EXECUTABLE.to_owned())?;
    let path = Path::new(executable);
    if !path.is_absolute() || !path.is_file() {
        return Err(INVALID_EXECUTABLE.to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let executable = path
            .metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
        if !executable {
            return Err(INVALID_EXECUTABLE.to_owned());
        }
    }
    #[cfg(windows)]
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
    {
        return Err(INVALID_EXECUTABLE.to_owned());
    }
    Ok(())
}

fn validate_public_url(value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(is_public_https_url) {
        Ok(())
    } else {
        Err(INVALID_PUBLIC_URL.to_owned())
    }
}

fn is_public_https_url(value: &str) -> bool {
    value.len() <= 2048
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
}

fn append_unique(headers: &mut Vec<String>, header: &str) {
    if !headers.iter().any(|existing| existing == header) {
        headers.push(header.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{
        build_profile, cloudflare_quick_profile, render_arguments, ProviderKind, ProviderProfile,
        ResolvedProvider, SaveProviderProfileInput,
    };

    #[test]
    fn custom_arguments_expand_without_shell_parsing() {
        let origin: SocketAddr = "127.0.0.1:43123".parse().expect("origin should parse");
        assert_eq!(
            render_arguments(
                &[
                    "serve".to_owned(),
                    "--url={origin}".to_owned(),
                    "; rm -rf /".to_owned(),
                    "{host}".to_owned(),
                    "{port}".to_owned(),
                ],
                origin,
            ),
            vec![
                "serve",
                "--url=http://127.0.0.1:43123",
                "; rm -rf /",
                "127.0.0.1",
                "43123",
            ]
        );
    }

    #[test]
    fn quick_profile_is_stable_and_needs_no_secret() {
        let profile = cloudflare_quick_profile();
        assert_eq!(profile.id, "cloudflare-quick");
        assert_eq!(profile.kind, ProviderKind::CloudflareQuick);
        assert!(!profile.kind.requires_credential(None));
    }

    #[test]
    fn custom_profile_rejects_a_relative_executable() {
        let error = build_profile(
            SaveProviderProfileInput {
                id: None,
                name: "Custom".to_owned(),
                kind: ProviderKind::Custom,
                executable: Some("vendor-cli".to_owned()),
                arguments: vec!["{origin}".to_owned()],
                public_url: None,
                url_pattern: Some(r"https://\S+".to_owned()),
                credential_env: None,
                forwarded_ip_header: None,
                local_port: None,
                credential: None,
                clear_credential: false,
            },
            None,
        )
        .expect_err("relative executable should be rejected");
        assert!(error.contains("executable"));
    }

    #[test]
    fn managed_cloudflare_keeps_its_token_out_of_process_arguments() {
        let profile = ProviderProfile {
            id: "managed".to_owned(),
            name: "Managed Cloudflare".to_owned(),
            kind: ProviderKind::CloudflareManaged,
            executable: None,
            arguments: Vec::new(),
            public_url: Some("https://share.example.com".to_owned()),
            url_pattern: None,
            credential_env: None,
            forwarded_ip_header: None,
            local_port: Some(43123),
            created_at: "now".to_owned(),
        };
        let launch = ResolvedProvider::new(profile, Some("secret-tunnel-token".to_owned()))
            .expect("managed provider should resolve")
            .launch("127.0.0.1:43123".parse().expect("origin should parse"))
            .expect("managed launch should build");

        assert_eq!(launch.arguments, ["tunnel", "--no-autoupdate", "run"]);
        assert!(!launch
            .arguments
            .iter()
            .any(|argument| argument.contains("secret-tunnel-token")));
        assert_eq!(
            launch.environment,
            [("TUNNEL_TOKEN".to_owned(), "secret-tunnel-token".to_owned())]
        );
        assert_eq!(
            launch
                .discovery
                .inspect("INF Registered tunnel connection connIndex=0"),
            Some("https://share.example.com".to_owned())
        );
    }

    #[test]
    fn managed_profile_discards_fields_that_only_custom_commands_use() {
        let profile = build_profile(
            SaveProviderProfileInput {
                id: None,
                name: "Managed Cloudflare".to_owned(),
                kind: ProviderKind::CloudflareManaged,
                executable: Some("/unused/vendor".to_owned()),
                arguments: vec!["--unused".to_owned()],
                public_url: Some("https://share.example.com".to_owned()),
                url_pattern: Some("unused".to_owned()),
                credential_env: Some("UNUSED_TOKEN".to_owned()),
                forwarded_ip_header: Some("X-Unused".to_owned()),
                local_port: Some(43123),
                credential: Some("never persisted here".to_owned()),
                clear_credential: false,
            },
            None,
        )
        .expect("managed profile should validate");

        assert_eq!(
            profile.public_url.as_deref(),
            Some("https://share.example.com")
        );
        assert_eq!(profile.local_port, Some(43123));
        assert!(profile.executable.is_none());
        assert!(profile.arguments.is_empty());
        assert!(profile.url_pattern.is_none());
        assert!(profile.credential_env.is_none());
        assert!(profile.forwarded_ip_header.is_none());
    }

    #[test]
    fn ngrok_uses_a_direct_executable_origin_and_secret_environment() {
        let executable = std::env::current_exe()
            .expect("test executable path should be available")
            .to_string_lossy()
            .into_owned();
        let profile = ProviderProfile {
            id: "ngrok".to_owned(),
            name: "ngrok".to_owned(),
            kind: ProviderKind::Ngrok,
            executable: Some(executable),
            arguments: Vec::new(),
            public_url: None,
            url_pattern: None,
            credential_env: None,
            forwarded_ip_header: None,
            local_port: None,
            created_at: "now".to_owned(),
        };
        let launch = ResolvedProvider::new(profile, Some("ngrok-secret".to_owned()))
            .expect("ngrok provider should resolve")
            .launch("127.0.0.1:4173".parse().expect("origin should parse"))
            .expect("ngrok launch should build");

        assert_eq!(&launch.arguments[..2], ["http", "http://127.0.0.1:4173"]);
        assert_eq!(
            launch.environment,
            [("NGROK_AUTHTOKEN".to_owned(), "ngrok-secret".to_owned())]
        );
        assert_eq!(
            launch
                .discovery
                .inspect(r#"{\"url\":\"https://calm-otter.ngrok-free.app\"}"#),
            Some("https://calm-otter.ngrok-free.app".to_owned())
        );
    }

    #[test]
    fn custom_provider_can_use_a_ready_message_with_a_fixed_url() {
        let executable = std::env::current_exe()
            .expect("test executable path should be available")
            .to_string_lossy()
            .into_owned();
        let profile = ProviderProfile {
            id: "custom".to_owned(),
            name: "Vendor CLI".to_owned(),
            kind: ProviderKind::Custom,
            executable: Some(executable),
            arguments: vec!["serve".to_owned(), "{origin}".to_owned()],
            public_url: Some("https://share.vendor.example".to_owned()),
            url_pattern: Some("READY".to_owned()),
            credential_env: Some("VENDOR_TOKEN".to_owned()),
            forwarded_ip_header: Some("X-Vendor-IP".to_owned()),
            local_port: None,
            created_at: "now".to_owned(),
        };
        let provider = ResolvedProvider::new(profile, Some("vendor-secret".to_owned()))
            .expect("custom provider should resolve");
        let launch = provider
            .launch("127.0.0.1:48123".parse().expect("origin should parse"))
            .expect("custom launch should build");

        assert_eq!(launch.arguments, ["serve", "http://127.0.0.1:48123"]);
        assert_eq!(
            launch.environment,
            [("VENDOR_TOKEN".to_owned(), "vendor-secret".to_owned())]
        );
        assert_eq!(
            launch.discovery.inspect("provider READY on edge"),
            Some("https://share.vendor.example".to_owned())
        );
        assert_eq!(provider.visitor_headers()[0], "x-vendor-ip".to_owned());
    }
}
