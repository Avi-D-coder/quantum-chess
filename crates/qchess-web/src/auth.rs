//! OAuth 2.0 / OIDC sign-in: GitHub, Google, Lichess and Apple.
//!
//! This module is framework-independent: `start` builds the provider URL and
//! remembers the PKCE verifier by `state`; `finish` exchanges the code and
//! returns the provider's account profile.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    db::now_ms,
    hub::{App, Config, PendingOAuth, random_id},
};

pub const PROVIDERS: [&str; 4] = ["github", "google", "lichess", "apple"];

pub fn label(provider: &str) -> &'static str {
    match provider {
        "github" => "GitHub",
        "google" => "Google",
        "lichess" => "Lichess",
        "apple" => "Apple",
        _ => "?",
    }
}

/// Providers whose environment variables are set.
pub fn enabled(cfg: &Config) -> Vec<&'static str> {
    PROVIDERS.iter().copied().filter(|p| configured(cfg, p)).collect()
}

fn configured(cfg: &Config, p: &str) -> bool {
    match p {
        "github" => cfg.get("GITHUB_CLIENT_ID").is_some() && cfg.get("GITHUB_CLIENT_SECRET").is_some(),
        "google" => cfg.get("GOOGLE_CLIENT_ID").is_some() && cfg.get("GOOGLE_CLIENT_SECRET").is_some(),
        "lichess" => cfg.get("LICHESS_CLIENT_ID").is_some(),
        "apple" => {
            ["APPLE_SERVICES_ID", "APPLE_TEAM_ID", "APPLE_KEY_ID", "APPLE_PRIVATE_KEY_PATH"]
                .iter()
                .all(|k| cfg.get(k).is_some())
        }
        _ => false,
    }
}

pub fn redirect_uri(cfg: &Config, provider: &str) -> String {
    format!("{}/auth/{provider}/callback", cfg.public_url)
}

fn client_id<'a>(cfg: &'a Config, p: &str) -> Option<&'a str> {
    match p {
        "github" => cfg.get("GITHUB_CLIENT_ID"),
        "google" => cfg.get("GOOGLE_CLIENT_ID"),
        "lichess" => cfg.get("LICHESS_CLIENT_ID"),
        "apple" => cfg.get("APPLE_SERVICES_ID"),
        _ => None,
    }
}

pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Only same-site relative paths are allowed as a post-login destination.
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_string(),
        _ => "/".to_string(),
    }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn query(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect::<Vec<_>>().join("&")
}

/// Builds the provider's authorization URL.
pub fn start(app: &App, provider: &str, next: Option<&str>) -> Option<String> {
    if !configured(&app.cfg, provider) {
        return None;
    }
    let cid = client_id(&app.cfg, provider)?;
    let state = random_id(32);
    let verifier = random_id(64);
    let redirect = redirect_uri(&app.cfg, provider);
    let challenge = pkce_challenge(&verifier);
    let url = match provider {
        "github" => format!(
            "https://github.com/login/oauth/authorize?{}",
            query(&[
                ("client_id", cid), ("redirect_uri", &redirect), ("scope", "read:user user:email"),
                ("state", &state), ("code_challenge", &challenge), ("code_challenge_method", "S256"),
            ])
        ),
        "google" => format!(
            "https://accounts.google.com/o/oauth2/v2/auth?{}",
            query(&[
                ("client_id", cid), ("redirect_uri", &redirect), ("response_type", "code"),
                ("scope", "openid email profile"), ("state", &state),
                ("code_challenge", &challenge), ("code_challenge_method", "S256"),
            ])
        ),
        "lichess" => format!(
            "https://lichess.org/oauth?{}",
            query(&[
                ("client_id", cid), ("redirect_uri", &redirect), ("response_type", "code"),
                ("state", &state), ("code_challenge", &challenge), ("code_challenge_method", "S256"),
            ])
        ),
        "apple" => format!(
            "https://appleid.apple.com/auth/authorize?{}",
            query(&[
                ("client_id", cid), ("redirect_uri", &redirect), ("response_type", "code"),
                ("response_mode", "form_post"), ("scope", "name email"), ("state", &state),
            ])
        ),
        _ => return None,
    };
    app.oauth.lock().unwrap_or_else(|e| e.into_inner()).insert(
        state,
        PendingOAuth { provider: provider.into(), verifier, next: safe_next(next), created_ms: now_ms() },
    );
    Some(url)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub subject: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug)]
pub struct AuthError(pub String);

impl<E: std::fmt::Display> From<E> for AuthError {
    fn from(e: E) -> Self {
        AuthError(e.to_string())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    id_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Token and profile endpoints of the providers (overridable in tests).
pub struct Endpoints {
    pub github_token: String,
    pub github_user: String,
    pub google_token: String,
    pub google_user: String,
    pub lichess_token: String,
    pub lichess_user: String,
    pub apple_token: String,
}

impl Endpoints {
    pub fn real() -> Endpoints {
        Endpoints {
            github_token: "https://github.com/login/oauth/access_token".into(),
            github_user: "https://api.github.com/user".into(),
            google_token: "https://oauth2.googleapis.com/token".into(),
            google_user: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            lichess_token: "https://lichess.org/api/token".into(),
            lichess_user: "https://lichess.org/api/account".into(),
            apple_token: "https://appleid.apple.com/auth/token".into(),
        }
    }
}

/// Checks `state`, exchanges `code` and loads the account profile.
/// Returns the profile and the page to continue to.
pub async fn finish(
    app: &App,
    provider: &str,
    code: &str,
    state: &str,
    apple_user: Option<&str>,
) -> Result<(Profile, String), AuthError> {
    finish_with(app, &Endpoints::real(), provider, code, state, apple_user).await
}

pub async fn finish_with(
    app: &App,
    ep: &Endpoints,
    provider: &str,
    code: &str,
    state: &str,
    apple_user: Option<&str>,
) -> Result<(Profile, String), AuthError> {
    let pending = app
        .oauth
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(state)
        .filter(|p| p.provider == provider)
        .ok_or_else(|| AuthError("sign-in expired, please try again".into()))?;
    let cfg = &app.cfg;
    let redirect = redirect_uri(cfg, provider);
    let cid = client_id(cfg, provider).ok_or_else(|| AuthError("provider not configured".into()))?;

    let tokens: TokenResponse = match provider {
        "github" => {
            app.http
                .post(&ep.github_token)
                .header("accept", "application/json")
                .form(&[
                    ("client_id", cid), ("client_secret", cfg.get("GITHUB_CLIENT_SECRET").unwrap_or("")),
                    ("code", code), ("redirect_uri", &redirect), ("code_verifier", &pending.verifier),
                ])
                .send().await?.json().await?
        }
        "google" => {
            app.http
                .post(&ep.google_token)
                .form(&[
                    ("client_id", cid), ("client_secret", cfg.get("GOOGLE_CLIENT_SECRET").unwrap_or("")),
                    ("code", code), ("redirect_uri", &redirect), ("grant_type", "authorization_code"),
                    ("code_verifier", &pending.verifier),
                ])
                .send().await?.json().await?
        }
        "lichess" => {
            app.http
                .post(&ep.lichess_token)
                .form(&[
                    ("client_id", cid), ("code", code), ("redirect_uri", &redirect),
                    ("grant_type", "authorization_code"), ("code_verifier", &pending.verifier),
                ])
                .send().await?.json().await?
        }
        "apple" => {
            let secret = apple_client_secret(cfg)?;
            app.http
                .post(&ep.apple_token)
                .form(&[
                    ("client_id", cid), ("client_secret", secret.as_str()), ("code", code),
                    ("redirect_uri", &redirect), ("grant_type", "authorization_code"),
                ])
                .send().await?.json().await?
        }
        _ => return Err(AuthError("unknown provider".into())),
    };
    if let Some(e) = tokens.error {
        return Err(AuthError(format!("{e}: {}", tokens.error_description.unwrap_or_default())));
    }

    let profile = match provider {
        "github" => {
            let token = tokens.access_token.ok_or_else(|| AuthError("no access token".into()))?;
            let u: Value = app.http.get(&ep.github_user)
                .bearer_auth(&token).header("accept", "application/vnd.github+json")
                .send().await?.json().await?;
            profile_github(&u)?
        }
        "google" => {
            let token = tokens.access_token.ok_or_else(|| AuthError("no access token".into()))?;
            let u: Value = app.http.get(&ep.google_user)
                .bearer_auth(&token).send().await?.json().await?;
            profile_google(&u)?
        }
        "lichess" => {
            let token = tokens.access_token.ok_or_else(|| AuthError("no access token".into()))?;
            let u: Value = app.http.get(&ep.lichess_user)
                .bearer_auth(&token).send().await?.json().await?;
            profile_lichess(&u)?
        }
        "apple" => {
            let id_token = tokens.id_token.ok_or_else(|| AuthError("no id_token".into()))?;
            profile_apple(&id_token, apple_user)?
        }
        _ => return Err(AuthError("unknown provider".into())),
    };
    Ok((profile, pending.next))
}

pub fn profile_github(u: &Value) -> Result<Profile, AuthError> {
    let id = u.get("id").and_then(Value::as_i64).ok_or_else(|| AuthError("bad GitHub profile".into()))?;
    let login = u.get("login").and_then(Value::as_str).unwrap_or("github-user");
    let name = u.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(login);
    Ok(Profile {
        subject: id.to_string(),
        display_name: login_like(login, name),
        avatar_url: u.get("avatar_url").and_then(Value::as_str).map(String::from),
        email: u.get("email").and_then(Value::as_str).map(String::from),
    })
}

pub fn profile_google(u: &Value) -> Result<Profile, AuthError> {
    let sub = u.get("sub").and_then(Value::as_str).ok_or_else(|| AuthError("bad Google profile".into()))?;
    let email = u.get("email").and_then(Value::as_str);
    let name = u
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| email.and_then(|e| e.split('@').next()))
        .unwrap_or("google-user");
    Ok(Profile {
        subject: sub.to_string(),
        display_name: name.to_string(),
        avatar_url: u.get("picture").and_then(Value::as_str).map(String::from),
        email: email.map(String::from),
    })
}

pub fn profile_lichess(u: &Value) -> Result<Profile, AuthError> {
    let id = u.get("id").and_then(Value::as_str).ok_or_else(|| AuthError("bad Lichess profile".into()))?;
    let name = u.get("username").and_then(Value::as_str).unwrap_or(id);
    Ok(Profile { subject: id.to_string(), display_name: name.to_string(), avatar_url: None, email: None })
}

pub fn profile_apple(id_token: &str, user_field: Option<&str>) -> Result<Profile, AuthError> {
    let claims = jwt_claims(id_token)?;
    let sub = claims.get("sub").and_then(Value::as_str).ok_or_else(|| AuthError("bad Apple token".into()))?;
    let email = claims.get("email").and_then(Value::as_str).map(String::from);
    // Name and email arrive only on the very first sign-in, in `user`.
    let mut name = None;
    if let Some(u) = user_field.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        let n = u.get("name");
        let first = n.and_then(|n| n.get("firstName")).and_then(Value::as_str).unwrap_or("");
        let last = n.and_then(|n| n.get("lastName")).and_then(Value::as_str).unwrap_or("");
        let full = format!("{first} {last}").trim().to_string();
        if !full.is_empty() {
            name = Some(full);
        }
    }
    let display_name = name
        .or_else(|| email.as_deref().and_then(|e| e.split('@').next()).map(String::from))
        .unwrap_or_else(|| "apple-user".into());
    Ok(Profile { subject: sub.to_string(), display_name, avatar_url: None, email })
}

fn login_like(login: &str, name: &str) -> String {
    if name == login { login.to_string() } else { name.to_string() }
}

/// Reads the claims of a JWT received directly from the provider's token
/// endpoint over TLS (so its signature need not be re-verified, per OIDC core
/// section 3.1.3.7).
fn jwt_claims(token: &str) -> Result<Value, AuthError> {
    let payload = token.split('.').nth(1).ok_or_else(|| AuthError("malformed token".into()))?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Apple's client secret: an ES256 JWT signed with the team's `.p8` key.
fn apple_client_secret(cfg: &Config) -> Result<String, AuthError> {
    let pem = std::fs::read(cfg.get("APPLE_PRIVATE_KEY_PATH").unwrap_or(""))?;
    let now = (now_ms() / 1000) as i64;
    let claims = json!({
        "iss": cfg.get("APPLE_TEAM_ID"), "iat": now, "exp": now + 300,
        "aud": "https://appleid.apple.com", "sub": cfg.get("APPLE_SERVICES_ID"),
    });
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = cfg.get("APPLE_KEY_ID").map(String::from);
    let key = jsonwebtoken::EncodingKey::from_ec_pem(&pem)?;
    Ok(jsonwebtoken::encode(&header, &claims, &key)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn next_must_be_relative() {
        assert_eq!(safe_next(Some("/game/abc")), "/game/abc");
        assert_eq!(safe_next(Some("//evil.com")), "/");
        assert_eq!(safe_next(Some("https://evil.com")), "/");
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn github_and_lichess_profiles() {
        let g = profile_github(&json!({"id": 7, "login": "avi", "name": "Avi D", "avatar_url": "u"})).unwrap();
        assert_eq!((g.subject.as_str(), g.display_name.as_str()), ("7", "Avi D"));
        let l = profile_lichess(&json!({"id": "bob", "username": "Bob"})).unwrap();
        assert_eq!(l.display_name, "Bob");
        let goo = profile_google(&json!({"sub": "1", "email": "a@b.c"})).unwrap();
        assert_eq!(goo.display_name, "a");
    }

    #[test]
    fn apple_profile_reads_token_and_first_login_name() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"001.x","email":"me@icloud.com"}"#);
        let token = format!("e30.{payload}.sig");
        let p = profile_apple(&token, Some(r#"{"name":{"firstName":"Ann","lastName":"Lee"}}"#)).unwrap();
        assert_eq!((p.subject.as_str(), p.display_name.as_str()), ("001.x", "Ann Lee"));
        let p = profile_apple(&token, None).unwrap();
        assert_eq!(p.display_name, "me");
    }

    #[test]
    fn query_encoding() {
        assert_eq!(query(&[("a", "b c"), ("d", "e&f")]), "a=b%20c&d=e%26f");
    }
}

#[cfg(test)]
#[path = "auth_mock_tests.rs"]
mod mock_tests;
