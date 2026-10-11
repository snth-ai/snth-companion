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

/// Parse `snth://pair?code=XXXXXX`. Returns the normalized 6-digit code.
/// Anything else (another scheme, another host, a missing or malformed
/// code) is `None`.
pub fn parse_deep_link(url: &str) -> Option<String> {
    let rest = url.strip_prefix("snth://")?;
    // host[/path]?query  -- we only accept the "pair" action.
    let (head, query) = match rest.split_once('?') {
        Some((h, q)) => (h, q),
        None => return None,
    };
    let action = head.split('/').next().unwrap_or("");
    if action != "pair" {
        return None;
    }
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("code=") {
            return normalize_code(v);
        }
    }
    None
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
}

impl Pending {
    pub fn from_claim(c: ClaimResult) -> Pending {
        Pending {
            synth_id: c.synth_id,
            synth_url: c.synth_url,
            token: c.companion_token,
        }
    }

    /// The WebSocket endpoint derived from the synth url: https -> wss,
    /// http -> ws, plus the connect path.
    pub fn ws_url(&self) -> String {
        let base = self.synth_url.trim_end_matches('/');
        let ws = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            base.to_string()
        };
        format!("{ws}/api/connect/ws")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_parsing() {
        assert_eq!(
            parse_deep_link("snth://pair?code=123456").as_deref(),
            Some("123456")
        );
        assert_eq!(
            parse_deep_link("snth://pair?x=1&code=12-34-56").as_deref(),
            Some("123456")
        );
        assert_eq!(parse_deep_link("snth://pair?code=12345"), None); // too short
        assert_eq!(parse_deep_link("snth://other?code=123456"), None); // wrong action
        assert_eq!(parse_deep_link("https://evil/pair?code=123456"), None); // wrong scheme
        assert_eq!(parse_deep_link("snth://pair"), None); // no code
    }

    #[test]
    fn ws_url_derivation() {
        let p = Pending {
            synth_id: "mia".into(),
            synth_url: "https://mia.snth.ai/".into(),
            token: "t".into(),
        };
        assert_eq!(p.ws_url(), "wss://mia.snth.ai/api/connect/ws");
        let p2 = Pending {
            synth_id: "x".into(),
            synth_url: "http://127.0.0.1:8080".into(),
            token: "t".into(),
        };
        assert_eq!(p2.ws_url(), "ws://127.0.0.1:8080/api/connect/ws");
    }
}
