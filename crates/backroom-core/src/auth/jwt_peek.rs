//! Reads the claims of a JWT access token **without verifying it**.
//!
//! Diagnostics only: soju validates tokens by introspection, and this client
//! never makes trust decisions from these claims. Useful to see what
//! Keycloak issued (audience, scopes, roles) when SASL fails.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct TokenClaims {
    pub issuer: Option<String>,
    pub subject: Option<String>,
    pub audience: Vec<String>,
    /// `azp`: the client the token was issued to.
    pub authorized_party: Option<String>,
    pub preferred_username: Option<String>,
    pub scope: Option<String>,
    pub issued_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub realm_roles: Vec<String>,
    /// `resource_access.<client>.roles`.
    pub client_roles: BTreeMap<String, Vec<String>>,
    /// All top-level claim names, to spot custom mappers.
    pub claim_names: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PeekError {
    #[error("token is not a JWT (expected three dot-separated parts)")]
    NotJwt,
    #[error("token payload is not valid base64url")]
    Base64,
    #[error("token payload is not a JSON object")]
    Json,
}

pub fn peek_claims(token: &str) -> Result<TokenClaims, PeekError> {
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(PeekError::NotJwt);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| PeekError::Base64)?;
    let Value::Object(claims) = serde_json::from_slice(&bytes).map_err(|_| PeekError::Json)? else {
        return Err(PeekError::Json);
    };

    let string = |k: &str| claims.get(k).and_then(Value::as_str).map(str::to_owned);
    let time = |k: &str| {
        claims
            .get(k)
            .and_then(Value::as_i64)
            .and_then(|t| DateTime::from_timestamp(t, 0))
    };
    let strings = |v: Option<&Value>| -> Vec<String> {
        match v {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    };

    let client_roles = claims
        .get("resource_access")
        .and_then(Value::as_object)
        .map(|clients| {
            clients
                .iter()
                .map(|(client, v)| (client.clone(), strings(v.get("roles"))))
                .collect()
        })
        .unwrap_or_default();

    Ok(TokenClaims {
        issuer: string("iss"),
        subject: string("sub"),
        audience: strings(claims.get("aud")),
        authorized_party: string("azp"),
        preferred_username: string("preferred_username"),
        scope: string("scope"),
        issued_at: time("iat"),
        expires_at: time("exp"),
        realm_roles: strings(claims.get("realm_access").and_then(|r| r.get("roles"))),
        client_roles,
        claim_names: claims.keys().cloned().collect(),
    })
}

impl fmt::Display for TokenClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".into());
        let list = |v: &[String]| {
            if v.is_empty() {
                "(none)".to_owned()
            } else {
                v.join(", ")
            }
        };
        writeln!(f, "  iss:                {}", opt(&self.issuer))?;
        writeln!(f, "  azp (client):       {}", opt(&self.authorized_party))?;
        writeln!(f, "  aud:                {}", list(&self.audience))?;
        writeln!(f, "  preferred_username: {}", opt(&self.preferred_username))?;
        writeln!(f, "  sub:                {}", opt(&self.subject))?;
        writeln!(f, "  scope:              {}", opt(&self.scope))?;
        if let Some(exp) = self.expires_at {
            let left = exp - Utc::now();
            let state = if left.num_seconds() > 0 {
                format!("in {}s", left.num_seconds())
            } else {
                format!("EXPIRED {}s ago", -left.num_seconds())
            };
            writeln!(f, "  exp:                {} ({state})", exp.to_rfc3339())?;
        }
        writeln!(f, "  realm roles:        {}", list(&self.realm_roles))?;
        for (client, roles) in &self.client_roles {
            writeln!(f, "  roles for {client}: {}", list(roles))?;
        }
        write!(f, "  claims present:     {}", self.claim_names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: &str) -> String {
        format!(
            "eyJhbGciOiJub25lIn0.{}.sig",
            URL_SAFE_NO_PAD.encode(payload)
        )
    }

    #[test]
    fn reads_keycloak_style_claims() {
        let token = jwt(
            r#"{"iss":"https://login.example/realms/r","aud":["soju","account"],"azp":"backroom",
                "preferred_username":"chris","scope":"openid soju_roles","exp":4102444800,
                "realm_access":{"roles":["default-roles-r"]},
                "resource_access":{"soju":{"roles":["access"]}}}"#,
        );
        let c = peek_claims(&token).unwrap();
        assert_eq!(c.authorized_party.as_deref(), Some("backroom"));
        assert_eq!(c.audience, ["soju", "account"]);
        assert_eq!(c.preferred_username.as_deref(), Some("chris"));
        assert_eq!(c.client_roles["soju"], ["access"]);
        assert_eq!(c.realm_roles, ["default-roles-r"]);
        assert!(c.expires_at.is_some());
        let shown = c.to_string();
        assert!(shown.contains("roles for soju: access"));
    }

    #[test]
    fn single_string_audience() {
        let c = peek_claims(&jwt(r#"{"aud":"soju"}"#)).unwrap();
        assert_eq!(c.audience, ["soju"]);
    }

    #[test]
    fn rejects_opaque_tokens() {
        assert!(matches!(
            peek_claims("opaque-token"),
            Err(PeekError::NotJwt)
        ));
        assert!(matches!(peek_claims("a.!!!.c"), Err(PeekError::Base64)));
    }
}
