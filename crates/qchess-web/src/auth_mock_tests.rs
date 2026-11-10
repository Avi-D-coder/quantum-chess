//! OAuth flows against a local mock provider: authorization URL, state and
//! PKCE checks, code exchange and profile loading for every provider.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;
use crate::{db::Db, hub::App};

/// Throwaway P-256 key generated for these tests only.
const TEST_P8: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgEE1DuRGVx69Eox/y
03+jo0lGeW7bElgnJzEfeSb1OD+hRANCAASxotV440HqmeYKkBVxE3hNGPhlruU/
phvqrPNKkOXyd45rLJXpIaQ7VGJsNH8ihU8WpdJh/9QOIeeK66zkF3g9
-----END PRIVATE KEY-----";

type Log = Arc<Mutex<Vec<(String, String, String)>>>; // (path, authorization, body)

async fn mock_provider() -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: Log = Arc::default();
    let log2 = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let log = log2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head, body) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 { return; }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_string();
                        let len = head.lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        while buf.len() < i + 4 + len {
                            let n = sock.read(&mut tmp).await.unwrap_or(0);
                            if n == 0 { break; }
                            buf.extend_from_slice(&tmp[..n]);
                        }
                        break (head, String::from_utf8_lossy(&buf[i + 4..]).to_string());
                    }
                };
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let auth = head.lines()
                    .find_map(|l| l.to_ascii_lowercase().starts_with("authorization:").then(|| l[14..].trim().to_string()))
                    .unwrap_or_default();
                log.lock().unwrap().push((path.clone(), auth.clone(), body));
                let bearer_ok = auth == "Bearer tok-123";
                let resp: Value = match path.as_str() {
                    "/github/token" | "/google/token" | "/lichess/token" => json!({ "access_token": "tok-123" }),
                    "/apple/token" => {
                        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"000123.apple","email":"pat@privaterelay.appleid.com"}"#);
                        json!({ "access_token": "x", "id_token": format!("eyJhbGciOiJFUzI1NiJ9.{payload}.sig") })
                    }
                    "/github/user" if bearer_ok => json!({ "id": 42, "login": "octo", "name": "Octo Cat", "avatar_url": "https://a/x.png" }),
                    "/google/user" if bearer_ok => json!({ "sub": "g-1", "name": "Gee", "email": "g@x.com", "picture": "p" }),
                    "/lichess/user" if bearer_ok => json!({ "id": "thibault", "username": "thibault" }),
                    _ => json!({ "error": "invalid_request", "error_description": "bad token or path" }),
                };
                let body = resp.to_string();
                let out = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(out.as_bytes()).await;
            });
        }
    });
    (base, log)
}

fn endpoints(base: &str) -> Endpoints {
    Endpoints {
        github_token: format!("{base}/github/token"),
        github_user: format!("{base}/github/user"),
        google_token: format!("{base}/google/token"),
        google_user: format!("{base}/google/user"),
        lichess_token: format!("{base}/lichess/token"),
        lichess_user: format!("{base}/lichess/user"),
        apple_token: format!("{base}/apple/token"),
    }
}

fn test_app(key_path: &str) -> App {
    let env: HashMap<String, String> = [
        ("GITHUB_CLIENT_ID", "gh-id"), ("GITHUB_CLIENT_SECRET", "gh-secret"),
        ("GOOGLE_CLIENT_ID", "go-id"), ("GOOGLE_CLIENT_SECRET", "go-secret"),
        ("LICHESS_CLIENT_ID", "li-id"),
        ("APPLE_SERVICES_ID", "com.example.web"), ("APPLE_TEAM_ID", "TEAM123"),
        ("APPLE_KEY_ID", "KEY456"), ("APPLE_PRIVATE_KEY_PATH", key_path),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    App::new(Db::open(":memory:").unwrap(), Config { public_url: "https://chess.example".into(), env })
}

fn param(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.replace("%2F", "/").replace("%3A", ":").replace("%20", " "))
    })
}

fn form(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.replace("%2F", "/").replace("%3A", ":"))
    })
}

#[tokio::test]
async fn every_provider_signs_in_against_mock() {
    let key_path = std::env::temp_dir().join(format!("qchess-test-{}.p8", std::process::id()));
    std::fs::write(&key_path, TEST_P8).unwrap();
    let app = test_app(key_path.to_str().unwrap());
    assert_eq!(enabled(&app.cfg), vec!["github", "google", "lichess", "apple"]);
    let (base, log) = mock_provider().await;
    let ep = endpoints(&base);

    let cases = [
        ("github", "https://github.com/login/oauth/authorize", "42", "Octo Cat"),
        ("google", "https://accounts.google.com/o/oauth2/v2/auth", "g-1", "Gee"),
        ("lichess", "https://lichess.org/oauth", "thibault", "thibault"),
        ("apple", "https://appleid.apple.com/auth/authorize", "000123.apple", "Pat Lee"),
    ];
    for (provider, authorize, subject, name) in cases {
        let url = start(&app, provider, Some("/game/abc")).unwrap();
        assert!(url.starts_with(authorize), "{url}");
        assert_eq!(param(&url, "redirect_uri").unwrap(), format!("https://chess.example/auth/{provider}/callback"));
        let state = param(&url, "state").unwrap();
        let verifier = app.oauth.lock().unwrap()[&state].verifier.clone();
        if provider == "apple" {
            assert_eq!(param(&url, "response_mode").as_deref(), Some("form_post"));
        } else {
            assert_eq!(param(&url, "code_challenge").unwrap(), pkce_challenge(&verifier));
        }

        let apple_user = (provider == "apple").then_some(r#"{"name":{"firstName":"Pat","lastName":"Lee"}}"#);
        let (profile, next) = finish_with(&app, &ep, provider, "the-code", &state, apple_user).await.unwrap();
        assert_eq!((profile.subject.as_str(), profile.display_name.as_str(), next.as_str()), (subject, name, "/game/abc"));

        let (_, _, body) = log.lock().unwrap().iter().find(|(p, _, _)| p == &format!("/{provider}/token")).cloned().unwrap();
        assert_eq!(form(&body, "code").as_deref(), Some("the-code"));
        if provider == "apple" {
            // The client secret is an ES256 JWT for our team and key.
            let jwt = form(&body, "client_secret").unwrap();
            let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(jwt.split('.').next().unwrap()).unwrap()).unwrap();
            assert_eq!((header["alg"].as_str(), header["kid"].as_str()), (Some("ES256"), Some("KEY456")));
            let claims = jwt_claims(&jwt).unwrap();
            assert_eq!((claims["iss"].as_str(), claims["sub"].as_str()), (Some("TEAM123"), Some("com.example.web")));
        } else {
            assert_eq!(form(&body, "code_verifier"), Some(verifier));
        }

        // A state can be used once.
        assert!(finish_with(&app, &ep, provider, "the-code", &state, None).await.is_err());
    }

    // State issued for one provider is rejected by another.
    let url = start(&app, "github", None).unwrap();
    let state = param(&url, "state").unwrap();
    assert!(finish_with(&app, &ep, "google", "c", &state, None).await.is_err());
    // An unknown state is rejected.
    assert!(finish_with(&app, &ep, "github", "c", "forged", None).await.is_err());
    let _ = std::fs::remove_file(key_path);
}

#[tokio::test]
async fn provider_error_is_reported() {
    let app = test_app("/nonexistent");
    let (base, _) = mock_provider().await;
    let mut ep = endpoints(&base);
    ep.github_token = format!("{base}/nope");
    let url = start(&app, "github", None).unwrap();
    let state = param(&url, "state").unwrap();
    let err = finish_with(&app, &ep, "github", "c", &state, None).await.unwrap_err();
    assert!(err.0.contains("invalid_request"), "{}", err.0);
    // A missing Apple key file makes the exchange fail cleanly, not panic.
    let url = start(&app, "apple", None).unwrap();
    let state = param(&url, "state").unwrap();
    assert!(finish_with(&app, &ep, "apple", "c", &state, None).await.is_err());
}
