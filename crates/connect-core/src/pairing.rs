//! Pairing: claim a 6-digit code against the hub, then confirm before the
//! token is kept.
//!
//! The deep link `snth://pair?code=XXXXXX` can be opened by any web page,
//! so claiming a code must never silently link this computer to a synth.
//! The flow is two steps the UI drives:
//!
//!  1. `begin` claims the code and holds the result in memory ONLY: synth
//!     url, synth id and the token. Nothing is stored and no link is
//!     opened yet.
//!  2. `confirm` is called after the owner saw the synth identity and said
//!     yes: only then is the token written to the SecretStore.
//!
//! The hub returns a synth id (not a display name), so the confirmation
//! screen shows the synth id and url; the synth's friendly name only
//! arrives later in `hello_ack`. Noted for the UI.

use std::time::Duration;

use serde::Deserialize;

/// Request timeout for the single claim call.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(15);
/// A neutral agent string: never the owner's email or name (a previous
/// agent leaked an email in a header).
const USER_AGENT: &str = "snth-connect";

/// What the hub returns from `/api/companion/claim-pair`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaimResult {
    #[serde(rename = "synth_url")]
    pub synth_url: String,
    #[serde(rename = "synth_id")]
    pub synth_id: String,
    #[serde(rename = "companion_token")]
    pub companion_token: String,
}

#[derive(Debug)]
pub enum ClaimError {
    /// The code is not six digits.
    BadCode,
    /// 404 from the hub: unknown / already used code.
    NotFound,
    /// 410 from the hub: the code expired.
    Expired,
    /// Anything else (network, 5xx, bad body).
    Other(String),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::BadCode => f.write_str("the code must be six digits"),
            ClaimError::NotFound => {
                f.write_str("code not found (already used or never existed); ask for a fresh one")
            }
            ClaimError::Expired => f.write_str("code expired; ask the synth for a fresh one"),
            ClaimError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ClaimError {}

/// Keep only the digits of what the user typed / the deep link carried.
pub fn normalize_code(raw: &str) -> Option<String> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 6 {
        Some(digits)
    } else {
        None
    }
}

/// What a `snth://` link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// `snth://pair?code=XXXXXX`: a code from the synth's bot (normalized
    /// to six digits).
    Pair(String),
    /// `snth://login?code=...&state=...`: the platform's answer to the
    /// app's sign-in. `code` is `Err(reason)` for
    /// `snth://login?error=...&state=...` (the owner pressed Cancel).
    Login { code: Result<String, String>, state: String },
}

/// The longest login code or state the app accepts from a link.
const MAX_LOGIN_FIELD: usize = 256;

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// A login code or state: url-safe characters only, bounded.
fn login_field(v: &str) -> Option<String> {
    let ok = !v.is_empty()
        && v.len() <= MAX_LOGIN_FIELD
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    ok.then(|| v.to_string())
}

/// Parse a `snth://` link. Anything else (another scheme, another action,
/// a missing or malformed value) is `None`.
pub fn parse_deep_link(url: &str) -> Option<DeepLink> {
    let rest = url.strip_prefix("snth://")?;
    // host[/path]?query
    let (head, query) = rest.split_once('?')?;
    let action = head.split('/').next().unwrap_or("");
    match action {
        "pair" => normalize_code(query_value(query, "code")?).map(DeepLink::Pair),
        "login" => {
            let state = login_field(query_value(query, "state")?)?;
            let code = match (query_value(query, "code"), query_value(query, "error")) {
                (Some(c), _) => Ok(login_field(c)?),
                (None, Some(e)) => Err(login_field(e).unwrap_or_else(|| "error".into())),
                (None, None) => return None,
            };
            Some(DeepLink::Login { code, state })
        }
        _ => None,
    }
}

/// The 6-digit code of a `snth://pair` link, for a caller that handles
/// nothing else.
pub fn parse_pair_link(url: &str) -> Option<String> {
    match parse_deep_link(url)? {
        DeepLink::Pair(code) => Some(code),
        DeepLink::Login { .. } => None,
    }
}

/// Claims a code against a hub. Behind a trait so tests run offline.
pub trait HubClient: Send + Sync {
    fn claim(&self, hub_url: &str, code: &str) -> Result<ClaimResult, ClaimError>;
}

/// The real client: POST JSON over HTTPS (rustls).
pub struct UreqHubClient;

impl HubClient for UreqHubClient {
    fn claim(&self, hub_url: &str, code: &str) -> Result<ClaimResult, ClaimError> {
        let code = normalize_code(code).ok_or(ClaimError::BadCode)?;
        let base = hub_url.trim_end_matches('/');
        let url = format!("{base}/api/companion/claim-pair");
        let agent = ureq::AgentBuilder::new()
            .timeout(CLAIM_TIMEOUT)
            .user_agent(USER_AGENT)
            .build();
        let resp = agent.post(&url).send_json(ureq::json!({ "code": code }));
        match resp {
            Ok(r) => r
                .into_json::<ClaimResult>()
                .map_err(|e| ClaimError::Other(format!("parse hub response: {e}"))),
            Err(ureq::Error::Status(404, _)) => Err(ClaimError::NotFound),
            Err(ureq::Error::Status(410, _)) => Err(ClaimError::Expired),
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                Err(ClaimError::Other(format!("hub returned {code}: {body}")))
            }
            Err(ureq::Error::Transport(t)) => {
                Err(ClaimError::Other(format!("could not reach hub: {t}")))
            }
        }
    }
}

/// A claimed-but-not-confirmed pairing. Held in memory only; dropping it
/// forgets the token.
#[derive(Debug, Clone)]
pub struct Pending {
    pub synth_id: String,
    pub synth_url: String,
    pub(crate) token: String,
    /// The synth's name when the platform said it (empty from the hub).
    pub synth_name: String,
    /// How the token was obtained.
    pub source: crate::settings::LinkSource,
    /// The platform's hub id (account links), for a later re-mint.
    pub hub_id: String,
    /// A code redeemed through the signed-in account: did the platform
    /// tie the synth to the account (None for a code through the hub).
    pub linked_to_account: Option<bool>,
    /// Why not, in the platform's words ("synth_linked_elsewhere").
    pub reason: Option<String>,
}

/// The WebSocket endpoint of a synth url: https -> wss, http -> ws, plus
/// the connect path.
pub fn ws_url_of(synth_url: &str) -> String {
    let base = synth_url.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws}/api/connect/ws")
}

impl Pending {
    pub fn from_claim(c: ClaimResult) -> Pending {
        Pending {
            synth_id: c.synth_id,
            synth_url: c.synth_url,
            token: c.companion_token,
            synth_name: String::new(),
            source: crate::settings::LinkSource::Code,
            hub_id: String::new(),
            linked_to_account: None,
            reason: None,
        }
    }

    /// The WebSocket endpoint derived from the synth url.
    pub fn ws_url(&self) -> String {
        ws_url_of(&self.synth_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_parsing() {
        let pair = |u: &str| parse_pair_link(u);
        assert_eq!(pair("snth://pair?code=123456").as_deref(), Some("123456"));
        assert_eq!(pair("snth://pair?x=1&code=12-34-56").as_deref(), Some("123456"));
        assert_eq!(pair("snth://pair?code=12345"), None); // too short
        assert_eq!(pair("snth://other?code=123456"), None); // wrong action
        assert_eq!(pair("https://evil/pair?code=123456"), None); // wrong scheme
        assert_eq!(pair("snth://pair"), None); // no code
    }

    #[test]
    fn login_links_carry_a_code_or_an_error_and_always_a_state() {
        assert_eq!(
            parse_deep_link("snth://login?code=cac_0a1b&state=Zx-_9"),
            Some(DeepLink::Login { code: Ok("cac_0a1b".into()), state: "Zx-_9".into() })
        );
        assert_eq!(
            parse_deep_link("snth://login?error=denied&state=abc"),
            Some(DeepLink::Login { code: Err("denied".into()), state: "abc".into() })
        );
        // No state, an empty or odd one, no code and no error: nothing.
        assert_eq!(parse_deep_link("snth://login?code=cac_1"), None);
        assert_eq!(parse_deep_link("snth://login?code=cac_1&state="), None);
        assert_eq!(parse_deep_link("snth://login?code=cac_1&state=a%20b"), None);
        assert_eq!(parse_deep_link("snth://login?state=abc"), None);
        assert_eq!(parse_deep_link("snth://login?code=a<b&state=abc"), None);
        let long = "a".repeat(MAX_LOGIN_FIELD + 1);
        assert_eq!(parse_deep_link(&format!("snth://login?code={long}&state=abc")), None);
        // A pair link is not a login and the other way round.
        assert_eq!(parse_pair_link("snth://login?code=123456&state=abc"), None);
    }

    #[test]
    fn ws_url_derivation() {
        assert_eq!(ws_url_of("https://mia.snth.ai/"), "wss://mia.snth.ai/api/connect/ws");
        assert_eq!(ws_url_of("http://127.0.0.1:8080"), "ws://127.0.0.1:8080/api/connect/ws");
    }
}
