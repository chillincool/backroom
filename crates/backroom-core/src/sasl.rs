//! SASL credential encoding for the IRC `AUTHENTICATE` exchange.
//!
//! - PLAIN: RFC 4616
//! - OAUTHBEARER: RFC 7628
//! - Chunking: <https://ircv3.net/specs/extensions/sasl-3.1>

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

/// Maximum base64 bytes per `AUTHENTICATE` line.
const CHUNK: usize = 400;

/// Credentials for one SASL login. `Debug` never prints secrets.
#[derive(Clone)]
pub enum SaslCredentials {
    OAuthBearer {
        token: SecretString,
    },
    Plain {
        username: String,
        password: SecretString,
    },
}

impl fmt::Debug for SaslCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaslCredentials::OAuthBearer { .. } => f
                .debug_struct("OAuthBearer")
                .field("token", &"[redacted]")
                .finish(),
            SaslCredentials::Plain { username, .. } => f
                .debug_struct("Plain")
                .field("username", username)
                .field("password", &"[redacted]")
                .finish(),
        }
    }
}

impl SaslCredentials {
    pub fn mechanism(&self) -> &'static str {
        match self {
            SaslCredentials::OAuthBearer { .. } => "OAUTHBEARER",
            SaslCredentials::Plain { .. } => "PLAIN",
        }
    }

    /// The raw (pre-base64) client initial response.
    ///
    /// OAUTHBEARER sends an empty authzid (`n,,`): soju rejects the login if
    /// a username is given and differs from what token introspection returns.
    pub fn initial_response(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::new());
        match self {
            SaslCredentials::OAuthBearer { token } => {
                out.extend_from_slice(b"n,,\x01auth=Bearer ");
                out.extend_from_slice(token.expose_secret().as_bytes());
                out.extend_from_slice(b"\x01\x01");
            }
            SaslCredentials::Plain { username, password } => {
                out.push(0);
                out.extend_from_slice(username.as_bytes());
                out.push(0);
                out.extend_from_slice(password.expose_secret().as_bytes());
            }
        }
        out
    }

    /// The `AUTHENTICATE` arguments carrying the initial response.
    pub fn authenticate_args(&self) -> Vec<SecretString> {
        encode_authenticate(&self.initial_response())
    }
}

/// Base64-encodes `payload` and splits it into `AUTHENTICATE` arguments.
///
/// A final `+` is sent when the payload is empty or its encoding is an exact
/// multiple of 400 bytes, so the server knows the response is complete.
pub fn encode_authenticate(payload: &[u8]) -> Vec<SecretString> {
    if payload.is_empty() {
        return vec![SecretString::from("+")];
    }
    let encoded = Zeroizing::new(STANDARD.encode(payload));
    let mut args: Vec<SecretString> = encoded
        .as_bytes()
        .chunks(CHUNK)
        // base64 output is ASCII, so byte chunks are valid UTF-8.
        .map(|c| SecretString::from(String::from_utf8_lossy(c).into_owned()))
        .collect();
    if encoded.len().is_multiple_of(CHUNK) {
        args.push(SecretString::from("+"));
    }
    args
}

/// Decodes a server `AUTHENTICATE` challenge. Returns `None` for `+`.
pub fn decode_challenge(arg: &str) -> Option<Vec<u8>> {
    if arg == "+" {
        return None;
    }
    STANDARD.decode(arg).ok()
}

/// RFC 7628 §3.2.3: after an error challenge the client sends a single
/// `%x01` to finish the exchange (the server then replies 904).
pub const OAUTHBEARER_ERROR_ACK: &str = "AQ==";

#[cfg(test)]
mod tests {
    use super::*;

    fn expose(args: &[SecretString]) -> Vec<String> {
        args.iter().map(|a| a.expose_secret().to_owned()).collect()
    }

    #[test]
    fn oauthbearer_initial_response_bytes() {
        let creds = SaslCredentials::OAuthBearer {
            token: SecretString::from("tok.en"),
        };
        assert_eq!(creds.mechanism(), "OAUTHBEARER");
        assert_eq!(
            creds.initial_response().as_slice(),
            b"n,,\x01auth=Bearer tok.en\x01\x01"
        );
        assert_eq!(
            expose(&creds.authenticate_args()),
            [STANDARD.encode(b"n,,\x01auth=Bearer tok.en\x01\x01")]
        );
    }

    #[test]
    fn plain_initial_response_bytes() {
        let creds = SaslCredentials::Plain {
            username: "chris".into(),
            password: SecretString::from("hunter2"),
        };
        assert_eq!(creds.mechanism(), "PLAIN");
        assert_eq!(creds.initial_response().as_slice(), b"\0chris\0hunter2");
        assert_eq!(expose(&creds.authenticate_args()), ["AGNocmlzAGh1bnRlcjI="]);
    }

    #[test]
    fn debug_never_contains_secrets() {
        let creds = SaslCredentials::OAuthBearer {
            token: SecretString::from("super-secret-token"),
        };
        assert!(!format!("{creds:?}").contains("super-secret-token"));
        let creds = SaslCredentials::Plain {
            username: "u".into(),
            password: SecretString::from("super-secret-pass"),
        };
        let dbg = format!("{creds:?}");
        assert!(!dbg.contains("super-secret-pass"));
        assert!(dbg.contains("\"u\""));
    }

    #[test]
    fn empty_payload_is_plus() {
        assert_eq!(expose(&encode_authenticate(b"")), ["+"]);
    }

    #[test]
    fn chunking_boundaries() {
        // 300 raw bytes → exactly 400 base64 bytes → one chunk then "+".
        let args = expose(&encode_authenticate(&[b'a'; 300]));
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].len(), 400);
        assert_eq!(args[1], "+");

        // 299 raw bytes → 400 base64 bytes too (with padding).
        let args = expose(&encode_authenticate(&[b'a'; 299]));
        assert_eq!(args.len(), 2);
        assert_eq!(args[1], "+");

        // 297 raw bytes → 396 base64 bytes → a single short chunk.
        let args = expose(&encode_authenticate(&[b'a'; 297]));
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].len(), 396);

        // 301 raw bytes → 404 base64 bytes → 400 + 4, no "+".
        let args = expose(&encode_authenticate(&[b'a'; 301]));
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].len(), 400);
        assert_eq!(args[1].len(), 4);

        // 600 raw bytes → 800 base64 bytes → 400 + 400 + "+".
        let args = expose(&encode_authenticate(&[b'a'; 600]));
        assert_eq!(args.len(), 3);
        assert_eq!(args[2], "+");

        let rejoined: String = expose(&encode_authenticate(&[7u8; 1000]))
            .into_iter()
            .filter(|a| a != "+")
            .collect();
        assert_eq!(STANDARD.decode(rejoined).unwrap(), vec![7u8; 1000]);
    }

    #[test]
    fn decodes_challenges() {
        assert_eq!(decode_challenge("+"), None);
        assert_eq!(
            decode_challenge("eyJzdGF0dXMiOiJpbnZhbGlkX3Rva2VuIn0=").unwrap(),
            br#"{"status":"invalid_token"}"#
        );
    }
}
