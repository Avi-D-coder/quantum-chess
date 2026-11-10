//! Username/password sign-in for local testing (`--features test-auth`).
//! Not for production: there is no rate limiting or password reset.

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{app, auth_routes::sign_in};
use crate::{auth::safe_next, db::UsernameError};

#[derive(Deserialize)]
pub struct Creds {
    username: String,
    password: String,
    next: Option<String>,
}

fn back(next: &str, error: &str) -> SeeOther {
    see_other(format!("/login?error={}&next={}", error.replace(' ', "%20"), next.replace('&', "%26")))
}

#[route(POST "/test-auth/register")]
pub async fn register_submit(cx: &Cx, Form(c): Form<Creds>) -> Result<SeeOther> {
    let next = safe_next(c.next.as_deref());
    let name = c.username.trim();
    if c.password.len() < 4 {
        return Ok(back(&next, "Password must be at least 4 characters."));
    }
    let salt = SaltString::generate(&mut OsRng);
    let Ok(hash) = Argon2::default().hash_password(c.password.as_bytes(), &salt) else {
        return Ok(back(&next, "Could not hash the password."));
    };
    match app(cx).db.create_password_user(name, &hash.to_string()) {
        Ok(user) => sign_in(cx, &user, next).await,
        Err(UsernameError::Taken) => Ok(back(&next, "That username is taken.")),
        Err(UsernameError::Invalid) => Ok(back(&next, "Use 2-20 letters, digits, underscores or hyphens.")),
    }
}

#[route(POST "/test-auth/login")]
pub async fn login_submit(cx: &Cx, Form(c): Form<Creds>) -> Result<SeeOther> {
    let next = safe_next(c.next.as_deref());
    let ok = app(cx).db.password_login(c.username.trim()).filter(|(_, hash)| {
        PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(c.password.as_bytes(), &h).is_ok())
    });
    match ok {
        Some((user, _)) => sign_in(cx, &user, next).await,
        None => Ok(back(&next, "Wrong username or password.")),
    }
}
