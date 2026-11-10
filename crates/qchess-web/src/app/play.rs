//! Lobby, live games, challenges and their JSON API.

use qchess::Game;
use serde::Deserialize;
use serde_json::{Value, json};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::{
            Json,
            websocket::{Message, WebSocket, WebSocketUpgrade},
        },
        error::{RouterErrorExt, SeeOther, bad_request, forbidden, not_found, see_other},
        page, path_param,
        response::Response,
        route,
    },
    view::{View, view},
};

use super::{app, current_user};
use crate::{
    db::{User, now_ms},
    hub::{ActionError, App, Bc, Live},
    json::{clock_json, move_json, pieces_json, positions_json},
};
use std::sync::Arc;

topcoat::router::path_param!(id);
topcoat::router::path_param!(token);

fn action_err(e: ActionError) -> topcoat::router::error::BadRequestError {
    bad_request(e.to_string())
}

/// A signed-in user who has chosen a username.
pub async fn require_player(cx: &Cx) -> Result<User> {
    let user = current_user(cx).await?.ok_or_unauthorized()?;
    if user.username.is_none() {
        return Err(forbidden().into());
    }
    Ok(user)
}

// ---------------------------------------------------------------- pages

#[page("/")]
pub async fn lobby(cx: &Cx) -> Result<impl View> {
    let user = current_user(cx).await?;
    let name = user.as_ref().map(|u| u.name().to_string()).unwrap_or_default();
    let can_play = user.as_ref().is_some_and(|u| u.username.is_some());
    let needs_name = user.as_ref().is_some_and(|u| u.username.is_none());
    Ok(view! {
        <div id="lobby" data-user=(name) data-can-play=(can_play.to_string())>
            if needs_name {
                <p class="notice"><a href="/welcome">"Choose a username"</a>" to start playing."</p>
            }
            <noscript>"The lobby needs JavaScript."</noscript>
        </div>
        <script src="/assets/lobby.js"></script>
    })
}

#[page("/game/{id}")]
pub async fn game(cx: &Cx) -> Result<impl View> {
    let id = path_param::<Id>(cx).to_string();
    let a = app(cx);
    let live = a.live(&id).is_some();
    if !live && a.db.game(&id).is_none() {
        return Err(not_found().into());
    }
    let ws = format!("/game/{id}/ws");
    let replay = format!("/api/game/{id}/state");
    Ok(view! {
        <link rel="stylesheet" href="/assets/board.css">
        if live {
            <div id="qgame" data-game-id=(id.clone()) data-ws-url=(ws)></div>
        } else {
            <div id="qgame" data-game-id=(id.clone()) data-replay-url=(replay)></div>
        }
        <script src="/assets/board.js"></script>
    })
}

#[page("/challenge/{token}")]
pub async fn challenge(cx: &Cx) -> Result<impl View> {
    let token = path_param::<Token>(cx).to_string();
    let user = current_user(cx).await?;
    let a = app(cx);
    let info = a.challenge_info(&token);
    let found = info.is_some();
    let (creator, tc, rated, creator_id) = info.unwrap_or_else(|| (String::new(), qchess_play::PRESETS[0], false, -1));
    let mine = user.as_ref().is_some_and(|u| u.id == creator_id);
    let anon = user.is_none();
    let needs_name = user.as_ref().is_some_and(|u| u.username.is_none());
    let tc_label = tc.label();
    let kind = if rated { "rated" } else { "casual" };
    let url = format!("{}/challenge/{token}", a.cfg.public_url);
    let accept = format!("/challenge/{token}/accept");
    let cancel = format!("/challenge/{token}/cancel");
    let login = format!("/login?next=/challenge/{token}");
    Ok(view! {
        <section class="card narrow">
          if !found {
            <h1>"Challenge not found"</h1>
            <p>"This challenge was accepted, cancelled or has expired."</p>
            <a class="btn" href="/">"Back to the lobby"</a>
          } else {
            <h1>"Quantum chess challenge"</h1>
            <p><b>(creator)</b>" invites you to a "(kind)" "<b>(tc_label)</b>" game."</p>
            if mine {
                <p>"Send this link to your friend. The game starts when they open it."</p>
                <input class="linkbox" type="text" readonly=(true) value=(url) onclick="this.select()">
                <div id="challenge-wait" class="muted">"Waiting for your friend..."</div>
                <form method="post" action=(cancel)><button class="btn danger" type="submit">"Cancel challenge"</button></form>
                <script src="/assets/lobby.js"></script>
            } else if anon {
                <a class="btn" href=(login)>"Sign in to accept"</a>
            } else if needs_name {
                <a class="btn" href="/welcome">"Choose a username first"</a>
            } else {
                <form method="post" action=(accept)><button class="btn" type="submit">"Accept and play"</button></form>
            }
          }
        </section>
    })
}

#[route(POST "/challenge/{token}/accept")]
pub async fn challenge_accept(cx: &Cx) -> Result<SeeOther> {
    let token = path_param::<Token>(cx).to_string();
    let user = require_player(cx).await?;
    let game_id = app(cx).accept_challenge(&user, &token).map_err(action_err)?;
    Ok(see_other(format!("/game/{game_id}")))
}

#[route(POST "/challenge/{token}/cancel")]
pub async fn challenge_cancel(cx: &Cx) -> Result<SeeOther> {
    let token = path_param::<Token>(cx).to_string();
    let user = require_player(cx).await?;
    let _ = app(cx).cancel_challenge(&user, &token);
    Ok(see_other("/"))
}

#[route(GET "/tv")]
pub async fn tv(cx: &Cx) -> Result<SeeOther> {
    Ok(match app(cx).tv_game() {
        Some(id) => see_other(format!("/game/{id}")),
        None => see_other("/"),
    })
}

// ---------------------------------------------------------------- websocket

#[route(GET "/game/{id}/ws")]
pub async fn game_ws(cx: &Cx, upgrade: WebSocketUpgrade) -> Result<Response> {
    let id = path_param::<Id>(cx).to_string();
    let a = app(cx).clone();
    let live = a.live(&id).ok_or_not_found()?;
    let user = current_user(cx).await?;
    upgrade.on_upgrade(move |socket| run_socket(a, live, user, socket))
}

pub async fn send(socket: &mut WebSocket, v: &Value) -> bool {
    socket.send(Message::text(v.to_string())).await.is_ok()
}

pub async fn run_socket(app: Arc<App>, live: Arc<Live>, user: Option<User>, mut socket: WebSocket) {
    let viewer = user.as_ref().map(|u| u.id);
    let mut rx = live.tx.subscribe();
    if !send(&mut socket, &app.snapshot_json(&live, viewer)).await {
        return;
    }
    for line in app.chat_history(&live) {
        if !send(&mut socket, &line).await {
            return;
        }
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                if let Message::Text(text) = msg {
                    let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if let Some(reply) = app.client_message(&live, user.as_ref(), &v)
                        && !send(&mut socket, &reply).await
                    {
                        break;
                    }
                }
            }
            event = rx.recv() => {
                use tokio::sync::broadcast::error::RecvError;
                let ok = match event {
                    Ok(Bc::State) | Err(RecvError::Lagged(_)) => send(&mut socket, &app.snapshot_json(&live, viewer)).await,
                    Ok(Bc::Chat(v)) => send(&mut socket, &v).await,
                    Err(RecvError::Closed) => false,
                };
                if !ok { break; }
            }
        }
    }
}

// ---------------------------------------------------------------- JSON API

#[route(GET "/api/lobby")]
pub async fn api_lobby(cx: &Cx) -> Result<Json<Value>> {
    let user = current_user(cx).await?;
    Ok(Json(app(cx).lobby_json(user.as_ref())))
}

#[derive(Deserialize)]
struct SeekReq {
    tc: String,
    #[serde(default)]
    rated: bool,
    #[serde(default = "random")]
    color: String,
}

fn random() -> String {
    "random".into()
}

#[route(POST "/api/seek")]
pub async fn api_seek(cx: &Cx, Json(r): Json<SeekReq>) -> Result<Json<Value>> {
    let user = require_player(cx).await?;
    app(cx).create_seek(&user, &r.tc, r.rated, &r.color).map_err(action_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct IdReq {
    id: String,
}

#[route(POST "/api/seek/cancel")]
pub async fn api_seek_cancel(cx: &Cx, Json(r): Json<IdReq>) -> Result<Json<Value>> {
    let user = require_player(cx).await?;
    app(cx).cancel_seek(&user, &r.id).map_err(action_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[route(POST "/api/seek/accept")]
pub async fn api_seek_accept(cx: &Cx, Json(r): Json<IdReq>) -> Result<Json<Value>> {
    let user = require_player(cx).await?;
    let gid = app(cx).accept_seek(&user, &r.id).map_err(action_err)?;
    Ok(Json(json!({ "game": gid })))
}

#[route(POST "/api/challenge")]
pub async fn api_challenge(cx: &Cx, Json(r): Json<SeekReq>) -> Result<Json<Value>> {
    let user = require_player(cx).await?;
    let a = app(cx);
    let token = a.create_challenge(&user, &r.tc, r.rated, &r.color).map_err(action_err)?;
    Ok(Json(json!({ "token": token, "url": format!("/challenge/{token}") })))
}

#[derive(Deserialize)]
struct BotReq {
    level: String,
    tc: String,
    #[serde(default = "random")]
    color: String,
}

/// Start a casual game against the computer.
#[route(POST "/api/bot")]
pub async fn api_bot(cx: &Cx, Json(r): Json<BotReq>) -> Result<Json<Value>> {
    let user = require_player(cx).await?;
    let gid = app(cx).start_bot_game(&user, &r.level, &r.tc, &r.color).map_err(action_err)?;
    Ok(Json(json!({ "game": gid })))
}

/// The start position and the position after every move so far, for
/// stepping back through a game in progress (or a finished one).
#[route(GET "/api/game/{id}/positions")]
pub async fn api_positions(cx: &Cx) -> Result<Json<Value>> {
    let id = path_param::<Id>(cx).to_string();
    let a = app(cx);
    let history: Vec<String> = match a.live(&id) {
        Some(live) => live.room().game().history().to_vec(),
        None => {
            a.db.game(&id).ok_or_not_found()?;
            a.db.moves(&id).into_iter().map(|m| m.notation).collect()
        }
    };
    let positions = positions_json(&history).ok_or_else(|| bad_request("history does not replay"))?;
    Ok(Json(positions))
}

/// State of a finished game that is no longer in memory, rebuilt from the
/// database (including `positions` for replay).
#[route(GET "/api/game/{id}/state")]
pub async fn api_archived_game(cx: &Cx) -> Result<Json<Value>> {
    let id = path_param::<Id>(cx).to_string();
    let a = app(cx);
    let row = a.db.game(&id).ok_or_not_found()?;
    let moves = a.db.moves(&id);
    let history: Vec<String> = moves.iter().map(|m| m.notation.clone()).collect();
    let qgame = Game::from_history(&history).map_err(|e| bad_request(e.to_string()))?;
    let speed = qchess_play::TimeControl { initial_ms: row.initial_ms, increment_ms: row.increment_ms }.speed();
    let player = |uid: i64, name: &str| {
        let (r, n) = a.db.rating(uid, speed.name());
        json!({ "username": name, "rating": r.rating.round() as i64, "provisional": n < 10 })
    };
    let (w_ms, b_ms) = moves
        .last()
        .map(|m| (m.white_ms, m.black_ms))
        .unwrap_or((row.initial_ms, row.initial_ms));
    let clock = qchess_play::ClockView {
        initial_ms: row.initial_ms,
        increment_ms: row.increment_ms,
        white_ms: w_ms,
        black_ms: b_ms,
        running: None,
        server_now_ms: now_ms(),
    };
    let display = qgame.display_history();
    let move_rows: Vec<Value> = moves
        .iter()
        .enumerate()
        .map(|(i, m)| {
            move_json(&qchess_play::MoveRecord {
                ply: m.ply,
                notation: m.notation.clone(),
                display: display.get(i).cloned().unwrap_or_else(|| m.notation.clone()),
                collapses: m.collapses.clone(),
                white_ms: m.white_ms,
                black_ms: m.black_ms,
                at_ms: 0,
            })
        })
        .collect();
    Ok(Json(json!({
        "id": row.id,
        "you": null,
        "players": { "white": player(row.white_id, &row.white_name), "black": player(row.black_id, &row.black_name) },
        "clock": clock_json(&clock),
        "to_move": qgame.to_move().name(),
        "status": if row.status == "ended" { json!({ "kind": "ended", "result": row.result, "termination": row.termination }) } else { json!({ "kind": "ongoing" }) },
        "check": qchess_play::check_str(qgame.check()),
        "state_count": qgame.state_count(),
        "pieces": pieces_json(&qgame.pieces()),
        "targets": {},
        "last_move": qgame.last_move_squares(),
        "moves": move_rows,
        "draw_offer": null,
        "rated": row.rated,
        "positions": positions_json(&history),
    })))
}
