//! Player profiles: ratings, results and recent games.

use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::RouterErrorExt, page, path_param},
    view::{View, view},
};
use serde::Deserialize;

use super::app;
use crate::hub::rating_display;

topcoat::router::path_param!(username);

const PAGE: usize = 20;

#[derive(Deserialize, Default)]
pub struct PageQuery {
    page: Option<usize>,
}

struct Row {
    href: String,
    white: String,
    black: String,
    tc: String,
    result: String,
    outcome: &'static str,
    kind: &'static str,
    how: String,
}

#[page("/@/{username}")]
pub async fn profile(cx: &Cx, Form(q): Form<PageQuery>) -> Result<impl View> {
    let name = path_param::<Username>(cx).to_string();
    let a = app(cx);
    let user = a.db.user_by_name(&name).ok_or_not_found()?;
    let page = q.page.unwrap_or(0);
    let stats = a.db.stats(user.id);
    let ratings: Vec<(String, i64, u32)> = a
        .db
        .ratings(user.id)
        .into_iter()
        .map(|(s, r, n)| (s, rating_display(r), n))
        .collect();
    let games = a.db.user_games(user.id, PAGE + 1, page * PAGE);
    let more = games.len() > PAGE;
    let rows: Vec<Row> = games
        .into_iter()
        .take(PAGE)
        .map(|g| {
            let me_white = g.white_id == user.id;
            let result = g.result.clone().unwrap_or_default();
            let aborted = g.termination.as_deref() == Some("abort");
            let outcome = match (result.as_str(), me_white) {
                _ if aborted => "aborted",
                ("1-0", true) | ("0-1", false) => "won",
                ("1-0", false) | ("0-1", true) => "lost",
                _ => "drawn",
            };
            Row {
                href: format!("/game/{}", g.id),
                white: g.white_name,
                black: g.black_name,
                tc: qchess_play::TimeControl { initial_ms: g.initial_ms, increment_ms: g.increment_ms }.label(),
                result: if aborted { "-".into() } else { result },
                outcome,
                kind: if g.rated { "rated" } else { "casual" },
                how: g.termination.unwrap_or_default(),
            }
        })
        .collect();
    let has_ratings = !ratings.is_empty();
    let has_games = !rows.is_empty();
    let prev = (page > 0).then(|| format!("/@/{name}?page={}", page - 1));
    let next = more.then(|| format!("/@/{name}?page={}", page + 1));
    let avatar = user.avatar_url.clone();
    let title = user.name().to_string();
    let since = user.display_name.clone();
    Ok(view! {
        <section class="card profile">
            <div class="who">
                if let Some(src) = avatar {
                    <img class="avatar" src=(src) alt="" width="64" height="64">
                }
                <div>
                    <h1>(title)</h1>
                    <div class="muted">(since)</div>
                </div>
            </div>
            <div class="stats">
                <span class="stat"><b>(stats.wins)</b>" won"</span>
                <span class="stat"><b>(stats.draws)</b>" drawn"</span>
                <span class="stat"><b>(stats.losses)</b>" lost"</span>
            </div>
            <h2>"Ratings"</h2>
            if has_ratings {
                <div class="ratings">
                    for (speed, rating, n) in ratings {
                        <div class="rating"><span class="speed">(speed)</span><b>(rating)</b><small>(n)" games"</small></div>
                    }
                </div>
            } else {
                <p class="muted">"No rated games yet."</p>
            }
            <h2>"Games"</h2>
            if has_games {
                <table class="games">
                    <thead><tr><th>"White"</th><th>"Black"</th><th>"Time"</th><th>"Result"</th><th>"How"</th><th></th></tr></thead>
                    <tbody>
                        for r in rows {
                            <tr class=(r.outcome)>
                                <td>(r.white)</td><td>(r.black)</td>
                                <td>(r.tc)" "<small>(r.kind)</small></td>
                                <td>(r.result)" "<b>(r.outcome)</b></td>
                                <td>(r.how)</td>
                                <td><a href=(r.href)>"Replay"</a></td>
                            </tr>
                        }
                    </tbody>
                </table>
            } else {
                <p class="muted">"No finished games yet."</p>
            }
            <div class="pager">
                if let Some(p) = prev { <a class="btn" href=(p)>"Newer"</a> }
                if let Some(n) = next { <a class="btn" href=(n)>"Older"</a> }
            </div>
        </section>
    })
}
