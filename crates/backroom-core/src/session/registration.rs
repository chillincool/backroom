//! Connection registration: `CAP LS 302` → `CAP REQ` → SASL →
//! optional `BOUNCER BIND` → `CAP END` → welcome burst.

use std::collections::{BTreeMap, BTreeSet};

use secrecy::SecretString;

use crate::proto::Message;
use crate::proto::caps::{CapSet, chunk_cap_req};
use crate::proto::numerics::*;
use crate::sasl::{self, SaslCredentials};

#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    pub nick: String,
    pub username: String,
    pub realname: String,
    /// Capabilities to request if the server offers them.
    pub wanted_caps: Vec<String>,
    /// SASL is mandatory when set: registration fails rather than
    /// continuing unauthenticated.
    pub sasl: Option<SaslCredentials>,
    /// `soju.im/bouncer-networks` network ID to bind this connection to.
    pub bind_network: Option<String>,
}

/// A message for the driver to send.
pub enum Outgoing {
    Message(Message),
    /// An `AUTHENTICATE` line carrying credentials. Never log the argument.
    Authenticate(SecretString),
}

/// The result of successful registration.
#[derive(Debug, Clone, Default)]
pub struct Registered {
    pub nick: String,
    /// Account name from `RPL_LOGGEDIN`, if the server sent one.
    pub account: Option<String>,
    pub available_caps: CapSet,
    pub enabled_caps: BTreeSet<String>,
    /// `RPL_ISUPPORT` tokens; valueless tokens map to an empty string.
    pub isupport: BTreeMap<String, String>,
}

impl Registered {
    pub fn has_cap(&self, name: &str) -> bool {
        self.enabled_caps.contains(name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error(
        "server does not offer SASL {wanted} (sasl cap: {offered})",
        offered = offered.as_deref().unwrap_or("not advertised")
    )]
    SaslUnavailable {
        wanted: &'static str,
        offered: Option<String>,
    },
    #[error("SASL {mechanism} authentication failed: {line}")]
    SaslFailed {
        mechanism: &'static str,
        /// The server's exact failure line (904/905/906/…).
        line: String,
        /// RFC 7628 error challenge JSON, if the server sent one.
        challenge: Option<String>,
        /// Mechanisms from `RPL_SASLMECHS` (908), if sent.
        server_mechanisms: Option<String>,
    },
    #[error("server refused to bind to network: {line}")]
    BindFailed { line: String },
    #[error("server closed the connection: {line}")]
    ServerError { line: String },
    #[error("all nicknames rejected: {line}")]
    NickRejected { line: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    CapLs,
    CapReq,
    SaslStarted,
    SaslSent,
    Welcome,
    Done,
}

pub struct Registration {
    cfg: RegistrationConfig,
    state: State,
    out: Vec<Outgoing>,
    result: Registered,
    pending_acks: BTreeSet<String>,
    sasl_challenge: Option<String>,
    sasl_mechs: Option<String>,
    nick_attempts: u32,
}

impl Registration {
    pub fn new(cfg: RegistrationConfig) -> Self {
        let mut reg = Registration {
            state: State::CapLs,
            out: Vec::new(),
            result: Registered {
                nick: cfg.nick.clone(),
                ..Default::default()
            },
            pending_acks: BTreeSet::new(),
            sasl_challenge: None,
            sasl_mechs: None,
            nick_attempts: 0,
            cfg,
        };
        reg.send(Message::new("CAP", ["LS", "302"]));
        reg.send(Message::new("NICK", [reg.cfg.nick.clone()]));
        reg.send(Message::new(
            "USER",
            [
                reg.cfg.username.clone(),
                "0".into(),
                "*".into(),
                reg.cfg.realname.clone(),
            ],
        ));
        reg
    }

    /// Takes the messages queued for sending.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.out)
    }

    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Feeds one incoming message. `raw` is the line as received, used
    /// verbatim in error reports. Returns the result once registration
    /// finishes.
    pub fn handle(
        &mut self,
        msg: &Message,
        raw: &str,
    ) -> Result<Option<Registered>, RegistrationError> {
        match msg.command.as_str() {
            "PING" => {
                self.send(Message::new("PONG", msg.params.clone()));
            }
            "ERROR" => {
                return Err(RegistrationError::ServerError {
                    line: raw.to_owned(),
                });
            }
            "CAP" => self.handle_cap(msg)?,
            "AUTHENTICATE" if self.state == State::SaslStarted => self.handle_authenticate(msg),
            "AUTHENTICATE" if self.state == State::SaslSent => {
                // RFC 7628 error challenge: record it and finish the exchange.
                if let Some(bytes) = msg.param(0).and_then(sasl::decode_challenge) {
                    self.sasl_challenge = Some(String::from_utf8_lossy(&bytes).into_owned());
                }
                self.out.push(Outgoing::Authenticate(SecretString::from(
                    sasl::OAUTHBEARER_ERROR_ACK,
                )));
            }
            RPL_LOGGEDIN => {
                self.result.account = msg.param(2).map(str::to_owned);
            }
            RPL_SASLSUCCESS if self.state == State::SaslSent => self.finish_negotiation(),
            RPL_SASLMECHS => {
                self.sasl_mechs = msg.param(1).map(str::to_owned);
            }
            ERR_SASLFAIL | ERR_SASLTOOLONG | ERR_SASLABORTED | ERR_SASLALREADY | ERR_NICKLOCKED
                if matches!(self.state, State::SaslStarted | State::SaslSent) =>
            {
                return Err(RegistrationError::SaslFailed {
                    mechanism: self.sasl_mechanism(),
                    line: raw.to_owned(),
                    challenge: self.sasl_challenge.take(),
                    server_mechanisms: self.sasl_mechs.take(),
                });
            }
            "FAIL" if msg.param(0) == Some("BOUNCER") => {
                return Err(RegistrationError::BindFailed {
                    line: raw.to_owned(),
                });
            }
            ERR_NICKNAMEINUSE if self.state != State::Done => {
                self.nick_attempts += 1;
                if self.nick_attempts > 3 {
                    return Err(RegistrationError::NickRejected {
                        line: raw.to_owned(),
                    });
                }
                self.result.nick.push('_');
                self.send(Message::new("NICK", [self.result.nick.clone()]));
            }
            RPL_WELCOME => {
                if let Some(nick) = msg.param(0) {
                    self.result.nick = nick.to_owned();
                }
                self.state = State::Welcome;
            }
            RPL_ISUPPORT => {
                // params: <client> <token>... :are supported by this server
                let tokens = msg
                    .params
                    .iter()
                    .skip(1)
                    .take(msg.params.len().saturating_sub(2));
                for token in tokens {
                    if let Some(name) = token.strip_prefix('-') {
                        self.result.isupport.remove(name);
                    } else {
                        let (k, v) = token.split_once('=').unwrap_or((token, ""));
                        self.result.isupport.insert(k.to_owned(), v.to_owned());
                    }
                }
            }
            RPL_ENDOFMOTD | ERR_NOMOTD if self.state == State::Welcome => {
                self.state = State::Done;
                return Ok(Some(std::mem::take(&mut self.result)));
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_cap(&mut self, msg: &Message) -> Result<(), RegistrationError> {
        let sub = msg.param(1).unwrap_or_default().to_ascii_uppercase();
        match sub.as_str() {
            "LS" if self.state == State::CapLs => {
                // CAP <client> LS [*] :<caps>
                let more = msg.params.len() >= 4 && msg.param(2) == Some("*");
                if let Some(list) = msg.params.last() {
                    self.result.available_caps.extend_from_list(list);
                }
                if !more {
                    self.request_caps()?;
                }
            }
            "ACK" => {
                for name in msg.params.last().into_iter().flat_map(|l| l.split(' ')) {
                    if let Some(name) = name.strip_prefix('-') {
                        self.result.enabled_caps.remove(name);
                    } else if !name.is_empty() {
                        self.pending_acks.remove(name);
                        self.result.enabled_caps.insert(name.to_owned());
                    }
                }
                self.after_cap_reply()?;
            }
            "NAK" => {
                for name in msg.params.last().into_iter().flat_map(|l| l.split(' ')) {
                    self.pending_acks.remove(name);
                }
                self.after_cap_reply()?;
            }
            "NEW" => {
                if let Some(list) = msg.params.last() {
                    self.result.available_caps.extend_from_list(list);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn request_caps(&mut self) -> Result<(), RegistrationError> {
        if let Some(creds) = &self.cfg.sasl {
            let wanted = creds.mechanism();
            let caps = &self.result.available_caps;
            let mechs = caps.sasl_mechanisms();
            // A bare `sasl` cap (no value) doesn't list mechanisms; try anyway.
            if !caps.contains("sasl") || !(mechs.is_empty() || mechs.contains(&wanted)) {
                return Err(RegistrationError::SaslUnavailable {
                    wanted,
                    offered: caps.value("sasl").map(str::to_owned),
                });
            }
        }

        let mut req: Vec<&str> = self
            .cfg
            .wanted_caps
            .iter()
            .map(String::as_str)
            .filter(|c| self.result.available_caps.contains(c))
            .collect();
        if self.cfg.sasl.is_some() && !req.contains(&"sasl") {
            req.push("sasl");
        }

        self.state = State::CapReq;
        if req.is_empty() {
            self.finish_negotiation();
            return Ok(());
        }
        for line in chunk_cap_req(req.iter().copied()) {
            self.pending_acks.extend(line.split(' ').map(str::to_owned));
            self.send(Message::new("CAP", ["REQ".to_owned(), line]));
        }
        Ok(())
    }

    fn after_cap_reply(&mut self) -> Result<(), RegistrationError> {
        if self.state != State::CapReq || !self.pending_acks.is_empty() {
            return Ok(());
        }
        match &self.cfg.sasl {
            Some(creds) => {
                if !self.result.enabled_caps.contains("sasl") {
                    return Err(RegistrationError::SaslUnavailable {
                        wanted: creds.mechanism(),
                        offered: Some("sasl capability was rejected (CAP NAK)".into()),
                    });
                }
                let mech = creds.mechanism();
                self.send(Message::new("AUTHENTICATE", [mech]));
                self.state = State::SaslStarted;
            }
            None => self.finish_negotiation(),
        }
        Ok(())
    }

    fn handle_authenticate(&mut self, msg: &Message) {
        // The server's first reply to `AUTHENTICATE <mech>` is `+` for both
        // PLAIN and OAUTHBEARER; the client then sends its initial response.
        if msg.param(0) != Some("+") {
            tracing::debug!("unexpected non-empty initial SASL challenge; sending response anyway");
        }
        if let Some(creds) = &self.cfg.sasl {
            for arg in creds.authenticate_args() {
                self.out.push(Outgoing::Authenticate(arg));
            }
        }
        self.state = State::SaslSent;
    }

    fn finish_negotiation(&mut self) {
        if let Some(netid) = self.cfg.bind_network.clone() {
            self.send(Message::new("BOUNCER", ["BIND".to_owned(), netid]));
        }
        self.send(Message::new("CAP", ["END"]));
        self.state = State::Welcome;
    }

    fn sasl_mechanism(&self) -> &'static str {
        self.cfg
            .sasl
            .as_ref()
            .map_or("?", SaslCredentials::mechanism)
    }

    fn send(&mut self, msg: Message) {
        self.out.push(Outgoing::Message(msg));
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;
    use crate::proto::caps::WANTED_CAPS;

    fn cfg(sasl: Option<SaslCredentials>, bind: Option<&str>) -> RegistrationConfig {
        RegistrationConfig {
            nick: "chris".into(),
            username: "chris".into(),
            realname: "Backroom".into(),
            wanted_caps: WANTED_CAPS.iter().map(|s| s.to_string()).collect(),
            sasl,
            bind_network: bind.map(str::to_owned),
        }
    }

    fn bearer() -> Option<SaslCredentials> {
        Some(SaslCredentials::OAuthBearer {
            token: SecretString::from("TOKEN"),
        })
    }

    /// Feeds lines; returns everything sent, rendering AUTHENTICATE args.
    fn run(
        reg: &mut Registration,
        lines: &[&str],
    ) -> (Vec<String>, Result<Option<Registered>, RegistrationError>) {
        let mut sent = Vec::new();
        let drain = |reg: &mut Registration, sent: &mut Vec<String>| {
            for o in reg.take_outgoing() {
                sent.push(match o {
                    Outgoing::Message(m) => m.to_string(),
                    Outgoing::Authenticate(a) => format!("AUTHENTICATE {}", a.expose_secret()),
                });
            }
        };
        drain(reg, &mut sent);
        for line in lines {
            let msg = Message::parse(line).unwrap();
            match reg.handle(&msg, line) {
                Ok(Some(done)) => {
                    drain(reg, &mut sent);
                    return (sent, Ok(Some(done)));
                }
                Ok(None) => drain(reg, &mut sent),
                Err(e) => return (sent, Err(e)),
            }
        }
        (sent, Ok(None))
    }

    const LS: &str = ":soju CAP * LS * :batch cap-notify echo-message server-time draft/chathistory draft/read-marker";
    const LS_END: &str = ":soju CAP * LS :sasl=PLAIN,OAUTHBEARER soju.im/bouncer-networks soju.im/bouncer-networks-notify soju.im/webpush";

    #[test]
    fn oauthbearer_happy_path_with_bind() {
        let mut reg = Registration::new(cfg(bearer(), Some("42")));
        let ack = ":soju CAP * ACK :sasl batch server-time echo-message cap-notify draft/chathistory draft/read-marker soju.im/bouncer-networks";
        let (sent, res) = run(
            &mut reg,
            &[
                LS,
                LS_END,
                ack,
                "AUTHENTICATE +",
                ":soju 900 chris chris!chris@soju chris :You are now logged in as chris",
                ":soju 903 chris :SASL authentication successful",
                ":soju 001 chris :Welcome",
                ":soju 005 chris CHATHISTORY=1000 BOUNCER_NETID=42 CASEMAPPING=ascii :are supported",
                ":soju 422 chris :Use /motd to read the message of the day",
            ],
        );
        let done = res.unwrap().unwrap();
        let token_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            b"n,,\x01auth=Bearer TOKEN\x01\x01",
        );
        assert_eq!(
            sent,
            [
                "CAP LS 302".to_owned(),
                "NICK chris".into(),
                "USER chris 0 * Backroom".into(),
                "CAP REQ :sasl batch server-time echo-message cap-notify draft/chathistory draft/read-marker soju.im/bouncer-networks".into(),
                "AUTHENTICATE OAUTHBEARER".into(),
                format!("AUTHENTICATE {token_b64}"),
                "BOUNCER BIND 42".into(),
                "CAP END".into(),
            ]
        );
        assert_eq!(done.account.as_deref(), Some("chris"));
        assert!(done.has_cap("draft/chathistory"));
        assert!(!done.has_cap("soju.im/webpush"));
        assert_eq!(
            done.isupport.get("BOUNCER_NETID").map(String::as_str),
            Some("42")
        );
        assert_eq!(
            done.isupport.get("CHATHISTORY").map(String::as_str),
            Some("1000")
        );
    }

    #[test]
    fn sasl_failure_reports_exact_line() {
        let mut reg = Registration::new(cfg(bearer(), None));
        let (sent, res) = run(
            &mut reg,
            &[
                LS_END,
                ":soju CAP * ACK :sasl soju.im/bouncer-networks",
                "AUTHENTICATE +",
                ":soju 904 * :Invalid credentials",
            ],
        );
        assert!(!sent.iter().any(|l| l == "CAP END"));
        match res {
            Err(RegistrationError::SaslFailed {
                mechanism, line, ..
            }) => {
                assert_eq!(mechanism, "OAUTHBEARER");
                assert_eq!(line, ":soju 904 * :Invalid credentials");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn oauthbearer_error_challenge_is_acknowledged_and_reported() {
        let mut reg = Registration::new(cfg(bearer(), None));
        let (sent, res) = run(
            &mut reg,
            &[
                LS_END,
                ":soju CAP * ACK :sasl soju.im/bouncer-networks",
                "AUTHENTICATE +",
                "AUTHENTICATE eyJzdGF0dXMiOiJpbnZhbGlkX3Rva2VuIn0=",
                ":soju 904 * :Authentication failed",
            ],
        );
        assert!(sent.contains(&"AUTHENTICATE AQ==".to_owned()));
        match res {
            Err(RegistrationError::SaslFailed { challenge, .. }) => {
                assert_eq!(challenge.as_deref(), Some(r#"{"status":"invalid_token"}"#));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_mechanism_fails_before_sending_credentials() {
        let mut reg = Registration::new(cfg(bearer(), None));
        let (sent, res) = run(&mut reg, &[":srv CAP * LS :sasl=PLAIN batch"]);
        assert!(matches!(
            res,
            Err(RegistrationError::SaslUnavailable {
                wanted: "OAUTHBEARER",
                ..
            })
        ));
        assert!(!sent.iter().any(|l| l.starts_with("AUTHENTICATE")));
    }

    #[test]
    fn plain_without_sasl_cap_fails() {
        let creds = SaslCredentials::Plain {
            username: "u".into(),
            password: SecretString::from("p"),
        };
        let mut reg = Registration::new(cfg(Some(creds), None));
        let (_, res) = run(&mut reg, &[":srv CAP * LS :batch"]);
        assert!(matches!(
            res,
            Err(RegistrationError::SaslUnavailable { .. })
        ));
    }

    #[test]
    fn nak_of_optional_cap_continues() {
        let mut reg = Registration::new(cfg(None, None));
        let (sent, res) = run(
            &mut reg,
            &[
                ":srv CAP * LS :batch server-time",
                ":srv CAP * NAK :batch server-time",
                ":srv 001 chris :hi",
                ":srv 376 chris :End of MOTD",
            ],
        );
        assert!(sent.contains(&"CAP END".to_owned()));
        let done = res.unwrap().unwrap();
        assert!(done.enabled_caps.is_empty());
    }

    #[test]
    fn bind_failure_is_reported() {
        let mut reg = Registration::new(cfg(None, Some("99")));
        let (_, res) = run(
            &mut reg,
            &[
                ":srv CAP * LS :soju.im/bouncer-networks",
                ":srv CAP * ACK :soju.im/bouncer-networks",
                ":srv FAIL BOUNCER INVALID_NETID BIND 99 :Network not found",
            ],
        );
        match res {
            Err(RegistrationError::BindFailed { line }) => assert!(line.contains("INVALID_NETID")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn answers_ping_and_retries_nick() {
        let mut reg = Registration::new(cfg(None, None));
        let (sent, _) = run(
            &mut reg,
            &[
                "PING :abc",
                ":srv CAP * LS :",
                ":srv 433 * chris :Nickname is already in use",
            ],
        );
        assert!(sent.contains(&"PONG abc".to_owned()));
        assert!(sent.contains(&"NICK chris_".to_owned()));
    }
}
