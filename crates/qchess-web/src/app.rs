//! Router assembly, shared helpers and the page layout.

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    cookie::{Cookie, Cookies, RouterBuilderCookieExt, SameSite, cookies},
    router::{
        OriginPolicy, Router, Slot, layout, route,
        content::{Css, Js},
    },
    session::{self, RouterBuilderSessionExt, SessionConfig, Token, TokenStore, TokenStoreFuture},
    view::{View, view},
};

use crate::{db::User, hub::App};

mod auth_routes;
#[cfg(feature = "test-auth")]
mod test_auth;
mod play;
mod profile;

pub fn router(app: Arc<App>) -> Router {
    let app_cfg_https = app.cfg.public_url.starts_with("https://");
    let b = Router::builder()
        .app_context(app)
        .origin_policy(OriginPolicy::new().exempt_paths(["/auth/apple/callback"]))
        .cookies()
        .sessions(session_config(&app_cfg_https))
        .layout(root_layout)
        .page(play::lobby)
        .page(play::game)
        .route(play::game_ws)
        .page(play::challenge)
        .route(play::challenge_accept)
        .route(play::challenge_cancel)
        .route(play::tv)
        .route(play::api_lobby)
        .route(play::api_seek)
        .route(play::api_seek_cancel)
        .route(play::api_bot)
        .route(play::api_positions)
        .route(play::api_seek_accept)
        .route(play::api_challenge)
        .route(play::api_archived_game)
        .page(auth_routes::login)
        .route(auth_routes::start)
        .route(auth_routes::callback)
        .route(auth_routes::logout)
        .page(auth_routes::welcome)
        .route(auth_routes::welcome_submit)
        .page(profile::profile)
        .route(asset_board_js)
        .route(asset_board_css)
        .route(asset_lobby_js)
        .route(asset_site_css);
    #[cfg(feature = "test-auth")]
    let b = b.route(test_auth::login_submit).route(test_auth::register_submit);
    b.build()
}

/// Over HTTPS, Topcoat's default `__Host-session` cookie. Over plain HTTP
/// (local development), browsers like Safari reject `Secure` cookies, so use
/// an ordinary HttpOnly, SameSite=Lax cookie instead.
fn session_config(https: &bool) -> SessionConfig {
    if *https {
        SessionConfig::default()
    } else {
        SessionConfig::builder().token_store(PlainCookieStore).build()
    }
}

const PLAIN_COOKIE: &str = "qc_session";

struct PlainCookieStore;

impl TokenStore for PlainCookieStore {
    fn read<'a>(&'a self, cx: &'a Cx) -> TokenStoreFuture<'a, Option<Token>> {
        Box::pin(async move {
            Ok(cookies(cx).get(PLAIN_COOKIE).and_then(|c| Token::decode(c.value_trimmed()).ok()))
        })
    }

    fn write<'a>(&'a self, cx: &'a Cx, token: Token, max_age: std::time::Duration) -> TokenStoreFuture<'a, ()> {
        Box::pin(async move {
            let mut c = Cookie::new(PLAIN_COOKIE, token.encode());
            c.set_http_only(true);
            c.set_same_site(SameSite::Lax);
            c.set_path("/");
            c.set_max_age(topcoat::cookie::time::Duration::seconds(max_age.as_secs() as i64));
            cookies(cx).add(c);
            Ok(())
        })
    }

    fn delete<'a>(&'a self, cx: &'a Cx) -> TokenStoreFuture<'a, ()> {
        Box::pin(async move {
            let mut c = Cookie::new(PLAIN_COOKIE, "");
            c.set_path("/");
            cookies(cx).remove(c);
            Ok(())
        })
    }
}

pub fn app(cx: &Cx) -> &Arc<App> {
    app_context::<Arc<App>>(cx)
}

/// The signed-in user, if any.
pub async fn current_user(cx: &Cx) -> Result<Option<User>> {
    let Some(hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    Ok(app(cx).db.session_user(&hash[..]))
}

#[route(GET "/assets/board.js")]
async fn asset_board_js() -> Result<Js<&'static str>> {
    Ok(Js(include_str!("../assets/board.js")))
}

#[route(GET "/assets/board.css")]
async fn asset_board_css() -> Result<Css<&'static str>> {
    Ok(Css(include_str!("../assets/board.css")))
}

#[route(GET "/assets/lobby.js")]
async fn asset_lobby_js() -> Result<Js<&'static str>> {
    Ok(Js(include_str!("../assets/lobby.js")))
}

#[route(GET "/assets/site.css")]
async fn asset_site_css() -> Result<Css<&'static str>> {
    Ok(Css(include_str!("../assets/site.css")))
}

#[layout("/")]
async fn root_layout(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let user = current_user(cx).await?;
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width,initial-scale=1">
                <title>"Quantum Chess"</title>
                <link rel="stylesheet" href="/assets/site.css">
            </head>
            <body>
                <header class="site-nav">
                    <a class="brand" href="/">"Quantum Chess"</a>
                    <a href="/">"Play"</a>
                    <a href="/tv">"TV"</a>
                    <span class="spacer"></span>
                    if let Some(u) = &user {
                        <a href=(format!("/@/{}", u.name()))>(u.name().to_string())</a>
                        <form method="post" action="/logout" class="inline">
                            <button type="submit" class="linklike">"Sign out"</button>
                        </form>
                    } else {
                        <a href="/login">"Sign in"</a>
                    }
                </header>
                <main>(slot)</main>
            </body>
        </html>
    })
}
