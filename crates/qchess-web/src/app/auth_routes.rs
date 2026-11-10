//! Sign-in pages and OAuth endpoints.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, bad_request, not_found, see_other},
        page, path_param, route,
    },
    session,
    view::{View, view},
};

use super::{app, current_user};
use crate::{auth, db::UsernameError};

topcoat::router::path_param!(provider);

#[derive(Deserialize, Default)]
pub struct NextQuery {
    next: Option<String>,
    error: Option<String>,
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[page("/login")]
pub async fn login(cx: &Cx, Form(q): Form<NextQuery>) -> Result<impl View> {
    let next = auth::safe_next(q.next.as_deref());
    let providers: Vec<(String, &'static str)> = auth::enabled(&app(cx).cfg)
        .into_iter()
        .map(|p| (format!("/auth/{p}/start?next={}", encode(&next)), auth::label(p)))
        .collect();
    let pw = cfg!(feature = "test-auth");
    let none = providers.is_empty() && !pw;
    let next_for_pw = next.clone();
    let error = q.error.clone();
    Ok(view! {
        <section class="card narrow">
            <h1>"Sign in"</h1>
            <p>"Quantum Chess uses your existing account. There are no passwords here."</p>
            for (href, label) in providers {
                <a class="btn wide" href=(href)>"Continue with " (label)</a>
            }
            if pw {
                <form method="post" action="/test-auth/login" class="pwlogin">
                    <h2>"Username and password"</h2>
                    <p class="muted">"Test sign-in, enabled by the test-auth feature."</p>
                    if let Some(e) = error {
                        <p class="error">(e)</p>
                    }
                    <input type="hidden" name="next" value=(next_for_pw)>
                    <input type="text" name="username" placeholder="Username" minlength="2" maxlength="20" required=(true)>
                    <input type="password" name="password" placeholder="Password" minlength="4" required=(true)>
                    <div class="row">
                        <button type="submit" class="btn">"Sign in"</button>
                        <button type="submit" class="btn alt" formaction="/test-auth/register">"Register"</button>
                    </div>
                </form>
            }
            if none {
                <p class="muted">"No sign-in providers are configured. Set the environment variables listed in the README (for example GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET) and restart the server."</p>
            }
        </section>
    })
}

/// Creates a session for `user` and returns the post-login destination.
pub async fn sign_in(cx: &Cx, user: &crate::db::User, next: String) -> Result<SeeOther> {
    let a = app(cx);
    let sess = session::start(cx).await?;
    let expires = sess.expires_at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    a.db.create_session(&sess.token_hash[..], user.id, expires).map_err(|e| bad_request(e.to_string()))?;
    if user.username.is_none() {
        return Ok(see_other(format!("/welcome?next={}", encode(&next))));
    }
    Ok(see_other(next))
}

#[route(GET "/auth/{provider}/start")]
pub async fn start(cx: &Cx, Form(q): Form<NextQuery>) -> Result<SeeOther> {
    let provider = path_param::<Provider>(cx);
    let url = auth::start(app(cx), provider, q.next.as_deref()).ok_or_else(not_found)?;
    Ok(see_other(url))
}

#[derive(Deserialize, Default)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    user: Option<String>,
}

#[route([GET, POST] "/auth/{provider}/callback")]
pub async fn callback(cx: &Cx, Form(q): Form<Callback>) -> Result<SeeOther> {
    let provider = path_param::<Provider>(cx);
    if let Some(e) = q.error {
        return Err(bad_request(format!("sign-in was cancelled or failed: {e}")).into());
    }
    let (Some(code), Some(state)) = (q.code.as_deref(), q.state.as_deref()) else {
        return Err(bad_request("missing code or state").into());
    };
    let a = app(cx);
    let (profile, next) = auth::finish(a, provider, code, state, q.user.as_deref())
        .await
        .map_err(|e| bad_request(e.0))?;
    let user = a
        .db
        .upsert_identity(provider, &profile.subject, &profile.display_name, profile.avatar_url.as_deref(), profile.email.as_deref())
        .map_err(|e| bad_request(e.to_string()))?;
    sign_in(cx, &user, next).await
}

#[route(POST "/logout")]
pub async fn logout(cx: &Cx) -> Result<SeeOther> {
    if let Some(hash) = session::stop(cx).await? {
        app(cx).db.delete_session(&hash[..]);
    }
    Ok(see_other("/"))
}

fn suggest_username(display: &str) -> String {
    let s: String = display
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(20)
        .collect();
    if s.len() >= 2 { s } else { "player".into() }
}

#[derive(Deserialize, Default)]
pub struct WelcomeQuery {
    next: Option<String>,
    error: Option<String>,
}

#[page("/welcome")]
pub async fn welcome(cx: &Cx, Form(q): Form<WelcomeQuery>) -> Result<impl View> {
    let Some(user) = current_user(cx).await? else {
        return Err(see_other("/login").into());
    };
    if user.username.is_some() {
        return Err(see_other(auth::safe_next(q.next.as_deref())).into());
    }
    let next = auth::safe_next(q.next.as_deref());
    let suggestion = suggest_username(&user.display_name);
    let error = q.error.map(|e| match e.as_str() {
        "taken" => "That username is taken.".to_string(),
        _ => "Use 2-20 letters, digits, underscores or hyphens.".to_string(),
    });
    Ok(view! {
        <section class="card narrow">
            <h1>"Welcome!"</h1>
            <p>"Pick a username. It is public and cannot be changed later."</p>
            if let Some(e) = error {
                <p class="error">(e)</p>
            }
            <form method="post" action="/welcome">
                <input type="hidden" name="next" value=(next)>
                <input type="text" name="username" value=(suggestion) minlength="2" maxlength="20"
                    pattern="[A-Za-z0-9_\\-]+" required=(true) autofocus=(true)>
                <button type="submit" class="btn">"Start playing"</button>
            </form>
        </section>
    })
}

#[derive(Deserialize)]
struct WelcomeForm {
    username: String,
    next: Option<String>,
}

#[route(POST "/welcome")]
pub async fn welcome_submit(cx: &Cx, Form(f): Form<WelcomeForm>) -> Result<SeeOther> {
    let Some(user) = current_user(cx).await? else {
        return Ok(see_other("/login"));
    };
    let next = auth::safe_next(f.next.as_deref());
    match app(cx).db.set_username(user.id, f.username.trim()) {
        Ok(()) => Ok(see_other(next)),
        Err(UsernameError::Taken) => Ok(see_other(format!("/welcome?error=taken&next={}", encode(&next)))),
        Err(UsernameError::Invalid) => Ok(see_other(format!("/welcome?error=invalid&next={}", encode(&next)))),
    }
}
