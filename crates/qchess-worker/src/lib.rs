//! Quantum chess multiplayer on Cloudflare Workers.
//!
//! * The Worker serves the static site (`site/`, through the `ASSETS` binding)
//!   and forwards WebSockets and the game API to Durable Objects.
//! * [`LobbyObject`] (one instance, named `"lobby"`) holds open seeks, friend
//!   challenges and the list of live games.
//! * [`GameObject`] (one per game, named by game id) holds one
//!   [`qchess_play::Room`].
//!
//! Both use hibernatable WebSockets, so an idle game costs nothing while the
//! players think. A hibernated object loses its memory; the lobby is stored
//! as one JSON value and a game as its [`RoomInit`] plus its action log
//! ([`qchess_play::action`]), replayed on wake-up. Finished games are deleted
//! an hour after they end. There are no accounts: a browser picks a random
//! secret `pid` and a display name, and its player id is a hash of the `pid`.
//! The computer opponent runs in the browser (`qchess-wasm`), not here.

use std::cell::RefCell;

use qchess_play::{
    Action, ColorPref, Event, Lobby, LobbyError, Logged, PRESETS, PlayerInfo, Room, RoomInit, Seek, TimeControl,
    action::changed,
    json::{chat_json, client_action, collapse_choice, error_json, hide_ratings, positions_json, state_json},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::prelude::wasm_bindgen;
use worker::*;

/// An unstarted game (fewer than two plies) is aborted after this long without a move.
const ABORT_AFTER_MS: u64 = 60_000;
/// A finished game stays viewable this long, then is deleted.
const KEEP_ENDED_MS: u64 = 60 * 60_000;
/// Seeks older than this are dropped even if their owner is still connected.
const SEEK_MAX_AGE_MS: u64 = 2 * 60 * 60_000;
/// The lobby forgets live games older than this (in case an end notice was lost).
const GAME_LIST_MAX_AGE_MS: u64 = 6 * 60 * 60_000;

// ------------------------------------------------------------------ helpers

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = crypto, js_name = getRandomValues)]
    fn get_random_values(buf: &mut [u8]);
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    get_random_values(&mut b);
    b
}

/// A random id of `len` characters from `[A-Za-z0-9]`.
fn random_id(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in random_bytes::<32>() {
            // 248 = 4 * 62: reject the top values so every character is equally likely.
            if b < 248 && out.len() < len {
                out.push(ALPHABET[(b % 62) as usize] as char);
            }
        }
    }
    out
}

fn coin() -> bool {
    random_bytes::<1>()[0] & 1 == 1
}

fn now_ms() -> u64 {
    Date::now().as_millis()
}

fn valid_id(s: &str) -> bool {
    (4..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Who is on the other end of a socket. `id` is `None` for a spectator
/// without a `pid`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Who {
    id: Option<i64>,
    name: String,
}

impl Who {
    /// From the `pid` and `name` query parameters.
    fn from_url(url: &Url) -> Who {
        let mut pid = None;
        let mut name = String::new();
        for (k, v) in url.query_pairs() {
            match &*k {
                "pid" => pid = Some(v.into_owned()),
                "name" => name = v.into_owned(),
                _ => {}
            }
        }
        let id = pid
            .filter(|p| (16..=64).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            .map(|p| player_id(&p));
        Who { id, name: clean_name(&name) }
    }

    fn player(&self) -> Option<PlayerInfo> {
        Some(PlayerInfo { user_id: self.id?, username: self.name.clone(), rating: None })
    }
}

/// A stable, non-reversible id for a secret `pid`: 64-bit FNV-1a cut to 52
/// bits, since ids pass through JavaScript numbers (storage, socket attachments).
fn player_id(pid: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in pid.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h >> 12) as i64
}

/// Up to 20 letters, digits, spaces, `_` or `-`; "Anonymous" if nothing is left.
fn clean_name(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'))
        .take(20)
        .collect();
    let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if kept.is_empty() { "Anonymous".into() } else { kept }
}

fn send(ws: &WebSocket, v: &Value) {
    // A socket that closed meanwhile is not an error worth reporting.
    let _ = ws.send_with_str(v.to_string());
}

fn who_of(ws: &WebSocket) -> Who {
    ws.deserialize_attachment::<Who>()
        .ok()
        .flatten()
        .unwrap_or(Who { id: None, name: "Anonymous".into() })
}

fn is_upgrade(req: &Request) -> bool {
    req.headers().get("Upgrade").ok().flatten().is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

fn json_request(url: &str, body: &impl Serialize) -> Result<Request> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(serde_json::to_string(body).map_err(|e| Error::RustError(e.to_string()))?.into()));
    Request::new_with_init(url, &init)
}

/// Values are stored as JSON text: going through JavaScript objects would
/// turn maps into `Map`s and lose integers above 2^53.
async fn get_json<T: serde::de::DeserializeOwned>(state: &State, key: &str) -> Result<Option<T>> {
    match state.storage().get::<String>(key).await? {
        Some(s) => serde_json::from_str(&s).map(Some).map_err(|e| Error::RustError(e.to_string())),
        None => Ok(None),
    }
}

async fn put_json(state: &State, key: &str, json: serde_json::Result<String>) -> Result<()> {
    state.storage().put(key, json.map_err(|e| Error::RustError(e.to_string()))?).await
}

// ------------------------------------------------------------------ the Worker

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["ws", "lobby"] => env.durable_object("LOBBY")?.get_by_name("lobby")?.fetch_with_request(req).await,
        ["ws", "game", id] | ["api", "game", id, "positions"] if valid_id(id) => {
            env.durable_object("GAME")?.get_by_name(id)?.fetch_with_request(req).await
        }
        ["game", id] if valid_id(id) => page(&env, &url, "/game").await,
        ["challenge", token] if valid_id(token) => page(&env, &url, "/challenge").await,
        _ => Response::error("Not found", 404),
    }
}

/// A static page of `site/` served for a dynamic path.
async fn page(env: &Env, url: &Url, asset: &str) -> Result<Response> {
    env.assets("ASSETS")?.fetch(url.join(asset)?.to_string(), None).await
}

// ------------------------------------------------------------------ lobby

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LiveGame {
    id: String,
    white: String,
    black: String,
    tc: String,
    started_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LobbyData {
    lobby: Lobby,
    games: Vec<LiveGame>,
    /// Accepted challenges (token, game id, when), so the challenger's page
    /// can follow even if it was closed at the time.
    accepted: Vec<(String, String, u64)>,
}

/// Open seeks, challenges and live games. See the crate docs.
#[durable_object]
pub struct LobbyObject {
    state: State,
    env: Env,
    data: RefCell<Option<LobbyData>>,
}

impl DurableObject for LobbyObject {
    fn new(state: State, env: Env) -> Self {
        Self { state, env, data: RefCell::new(None) }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        if url.path() == "/ended" {
            // From a GameObject whose game just ended.
            let id = url.query_pairs().find(|(k, _)| k == "id").map(|(_, v)| v.into_owned()).unwrap_or_default();
            self.load().await?;
            self.with(|d| d.games.retain(|g| g.id != id));
            self.save().await?;
            self.broadcast();
            return Response::ok("ok");
        }
        if !is_upgrade(&req) {
            return Response::error("Expected a WebSocket", 426);
        }
        self.load().await?;
        let pair = WebSocketPair::new()?;
        self.state.accept_web_socket(&pair.server);
        pair.server.serialize_attachment(Who::from_url(&url))?;
        send(&pair.server, &self.view(&pair.server));
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(&self, ws: WebSocket, message: WebSocketIncomingMessage) -> Result<()> {
        let WebSocketIncomingMessage::String(text) = message else { return Ok(()) };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else { return Ok(()) };
        self.load().await?;
        let who = who_of(&ws);
        if let Err(e) = self.handle(&ws, &who, &msg).await {
            send(&ws, &error_json(e));
        }
        Ok(())
    }

    async fn websocket_close(&self, ws: WebSocket, _code: usize, _reason: String, _clean: bool) -> Result<()> {
        self.left(ws).await
    }

    async fn websocket_error(&self, ws: WebSocket, _error: Error) -> Result<()> {
        self.left(ws).await
    }
}

impl LobbyObject {
    /// A socket closed: a player's seeks go away with their last open lobby page.
    async fn left(&self, ws: WebSocket) -> Result<()> {
        self.load().await?;
        if let Some(id) = who_of(&ws).id {
            let still_here = self.state.get_websockets().iter().any(|s| s != &ws && who_of(s).id == Some(id));
            if !still_here && !self.with(|d| d.lobby.remove_user_seeks(id)).is_empty() {
                self.save().await?;
                self.broadcast();
            }
        }
        Ok(())
    }

    async fn load(&self) -> Result<()> {
        if self.data.borrow().is_some() {
            return Ok(());
        }
        let stored: Option<LobbyData> = get_json(&self.state, "data").await?;
        let mut data = self.data.borrow_mut();
        if data.is_none() {
            *data = Some(stored.unwrap_or_default());
        }
        Ok(())
    }

    async fn save(&self) -> Result<()> {
        let json = self.with(|d| serde_json::to_string(&*d));
        put_json(&self.state, "data", json).await
    }

    fn with<T>(&self, f: impl FnOnce(&mut LobbyData) -> T) -> T {
        let mut data = self.data.borrow_mut();
        let d = data.get_or_insert_with(LobbyData::default);
        // Housekeeping on every access is cheap and keeps stored data small.
        let now = now_ms();
        d.lobby.prune(now);
        let old: Vec<i64> = d
            .lobby
            .list()
            .iter()
            .filter(|s| now.saturating_sub(s.created_ms) > SEEK_MAX_AGE_MS)
            .map(|s| s.user_id)
            .collect();
        for u in old {
            d.lobby.remove_user_seeks(u);
        }
        d.games.retain(|g| now.saturating_sub(g.started_ms) < GAME_LIST_MAX_AGE_MS);
        d.accepted.retain(|(_, _, t)| now.saturating_sub(*t) < KEEP_ENDED_MS);
        f(d)
    }

    /// The `lobby` message as `ws` sees it.
    fn view(&self, ws: &WebSocket) -> Value {
        let me = who_of(ws).id;
        let online = self.state.get_websockets().len();
        self.with(|d| {
            let seeks: Vec<Value> = d
                .lobby
                .list()
                .iter()
                .map(|s| {
                    json!({ "id": s.id, "user": s.username, "tc": s.tc.label(), "color": s.color.as_str(),
                            "mine": Some(s.user_id) == me })
                })
                .collect();
            let games: Vec<Value> = d
                .games
                .iter()
                .rev()
                .map(|g| json!({ "id": g.id, "white": g.white, "black": g.black, "tc": g.tc }))
                .collect();
            let tcs: Vec<String> = PRESETS.iter().map(|t| t.label()).collect();
            json!({ "type": "lobby", "seeks": seeks, "games": games, "tcs": tcs, "online": online })
        })
    }

    fn broadcast(&self) {
        for ws in self.state.get_websockets() {
            send(&ws, &self.view(&ws));
        }
    }

    fn tell(&self, user: i64, v: &Value) {
        for ws in self.state.get_websockets() {
            if who_of(&ws).id == Some(user) {
                send(&ws, v);
            }
        }
    }

    async fn handle(&self, ws: &WebSocket, who: &Who, msg: &Value) -> std::result::Result<(), String> {
        let s = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let kind = s("type");
        if kind == "challenge_info" {
            send(ws, &self.challenge_info(who, &s("token")));
            return Ok(());
        }
        let me = who.player().ok_or("reload the page to play")?;
        let tc = || TimeControl::from_label(&s("tc")).ok_or("unknown time control");
        let color = || ColorPref::parse(&s("color")).ok_or("unknown color");
        let now = now_ms();
        let lobby_err = |e: LobbyError| e.to_string();
        match kind.as_str() {
            "seek" => {
                let seek = Seek {
                    id: random_id(8),
                    user_id: me.user_id,
                    username: me.username.clone(),
                    rating: None,
                    player_rating: None,
                    tc: tc()?,
                    rated: false,
                    color: color()?,
                    created_ms: now,
                };
                self.with(|d| d.lobby.add_seek(seek));
            }
            "cancel" => {
                self.with(|d| d.lobby.cancel_seek(me.user_id, &s("id"))).map_err(lobby_err)?;
            }
            "accept" => {
                let init =
                    self.with(|d| d.lobby.accept_seek(me, &s("id"), random_id(10), coin())).map_err(lobby_err)?;
                self.start_game(init).await?;
            }
            "challenge" => {
                let token = random_id(12);
                let (tc, color) = (tc()?, color()?);
                self.with(|d| d.lobby.create_challenge(me, tc, false, color, token.clone(), now));
                send(ws, &json!({ "type": "challenge", "token": token }));
            }
            "challenge_accept" => {
                let token = s("token");
                let init = self
                    .with(|d| d.lobby.accept_challenge(me, &token, random_id(10), coin(), now))
                    .map_err(lobby_err)?;
                let id = init.game_id.clone();
                self.start_game(init).await?;
                self.with(|d| d.accepted.push((token, id, now)));
            }
            "challenge_cancel" => {
                self.with(|d| d.lobby.cancel_challenge(me.user_id, &s("token"))).map_err(lobby_err)?;
                send(ws, &json!({ "type": "challenge_cancelled" }));
            }
            _ => return Err("unknown message".into()),
        }
        self.save().await.map_err(|e| e.to_string())?;
        self.broadcast();
        Ok(())
    }

    fn challenge_info(&self, who: &Who, token: &str) -> Value {
        self.with(|d| {
            if let Some((_, game, _)) = d.accepted.iter().find(|(t, _, _)| t == token) {
                return json!({ "type": "challenge_info", "found": true, "game": game });
            }
            match d.lobby.get_challenge(token) {
                Some(c) => json!({
                    "type": "challenge_info", "found": true, "creator": c.creator.username,
                    "tc": c.tc.label(), "color": c.color.as_str(), "mine": who.id == Some(c.creator.user_id),
                }),
                None => json!({ "type": "challenge_info", "found": false }),
            }
        })
    }

    /// Creates the game's Durable Object and sends both players there.
    async fn start_game(&self, init: RoomInit) -> std::result::Result<(), String> {
        let now = now_ms();
        let entry = LiveGame {
            id: init.game_id.clone(),
            white: init.white.username.clone(),
            black: init.black.username.clone(),
            tc: init.tc.label(),
            started_ms: now,
        };
        let players = [init.white.user_id, init.black.user_id];
        let meta = Meta { init, created_ms: now };
        let res = async {
            let stub = self.env.durable_object("GAME")?.get_by_name(&entry.id)?;
            let resp = stub.fetch_with_request(json_request("https://game/init", &meta)?).await?;
            if resp.status_code() != 200 {
                return Err(Error::RustError(format!("game init failed: {}", resp.status_code())));
            }
            Ok(())
        }
        .await;
        res.map_err(|e| e.to_string())?;
        let matched = json!({ "type": "matched", "game": entry.id });
        self.with(|d| d.games.push(entry));
        for p in players {
            self.tell(p, &matched);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ games

/// How a game was created; with the action log it rebuilds the room.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    init: RoomInit,
    created_ms: u64,
}

struct Loaded {
    meta: Meta,
    room: Room,
    log: Vec<Logged>,
    /// When the game ended, if it has.
    ended_ms: Option<u64>,
}

impl Loaded {
    fn last_activity_ms(&self) -> u64 {
        self.log.iter().filter(|l| matches!(l.a, Action::Move { .. })).map(|l| l.t).next_back().unwrap_or(self.meta.created_ms)
    }

    /// When the alarm should next fire.
    fn next_alarm_ms(&self, now: u64) -> u64 {
        if let Some(t) = self.ended_ms {
            return t + KEEP_ENDED_MS;
        }
        let mut next = self.room.next_deadline_ms(now).unwrap_or(u64::MAX);
        if self.room.plies() < 2 {
            next = next.min(self.last_activity_ms() + ABORT_AFTER_MS);
        }
        // Untimed games still get cleaned up eventually.
        next.min(now + 24 * 60 * 60_000)
    }
}

/// One live game. See the crate docs.
#[durable_object]
pub struct GameObject {
    state: State,
    env: Env,
    game: RefCell<Option<Loaded>>,
}

impl DurableObject for GameObject {
    fn new(state: State, env: Env) -> Self {
        Self { state, env, game: RefCell::new(None) }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let url = req.url()?;
        if url.path() == "/init" {
            let meta: Meta = req.json().await?;
            if self.load().await? {
                return Response::error("game exists", 409);
            }
            put_json(&self.state, "meta", serde_json::to_string(&meta)).await?;
            put_json(&self.state, "log", Ok("[]".to_string())).await?;
            let room = meta.init.clone().into_room(meta.created_ms);
            *self.game.borrow_mut() = Some(Loaded { meta, room, log: Vec::new(), ended_ms: None });
            self.schedule().await?;
            return Response::ok("ok");
        }
        if !self.load().await? {
            return Response::error("No such game (finished games are kept for an hour)", 404);
        }
        if url.path().ends_with("/positions") {
            let positions = self.with(|g| positions_json(g.room.game().history()));
            return match positions {
                Some(p) => Response::from_json(&p),
                None => Response::error("history does not replay", 500),
            };
        }
        if !is_upgrade(&req) {
            return Response::error("Expected a WebSocket", 426);
        }
        let pair = WebSocketPair::new()?;
        self.state.accept_web_socket(&pair.server);
        let who = Who::from_url(&url);
        send(&pair.server, &self.state_for(who.id));
        for line in self.with(|g| g.room.chat_log().iter().map(chat_json).collect::<Vec<_>>()) {
            send(&pair.server, &line);
        }
        pair.server.serialize_attachment(who)?;
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(&self, ws: WebSocket, message: WebSocketIncomingMessage) -> Result<()> {
        let WebSocketIncomingMessage::String(text) = message else { return Ok(()) };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else { return Ok(()) };
        if !self.load().await? {
            send(&ws, &error_json("this game no longer exists"));
            return Ok(());
        }
        let who = who_of(&ws);
        let Some(id) = who.id else {
            send(&ws, &error_json("spectators cannot play"));
            return Ok(());
        };
        let action = match client_action(&msg, id, &who.name) {
            Ok(a) => a,
            Err(e) => {
                send(&ws, &error_json(e));
                return Ok(());
            }
        };
        match self.act(action).await? {
            Ok(events) => {
                if let Some(choice) = collapse_choice(&events) {
                    send(&ws, &choice);
                }
            }
            Err(e) => send(&ws, &error_json(e)),
        }
        Ok(())
    }

    async fn websocket_close(&self, _ws: WebSocket, _code: usize, _reason: String, _clean: bool) -> Result<()> {
        Ok(())
    }

    async fn websocket_error(&self, _ws: WebSocket, _error: Error) -> Result<()> {
        Ok(())
    }

    async fn alarm(&self) -> Result<Response> {
        if !self.load().await? {
            self.state.storage().delete_all().await?;
            return Response::ok("gone");
        }
        let now = now_ms();
        if self.with(|g| g.ended_ms.is_some_and(|t| now >= t + KEEP_ENDED_MS)) {
            for ws in self.state.get_websockets() {
                let _ = ws.close(Some(1000), Some("game expired"));
            }
            *self.game.borrow_mut() = None;
            self.state.storage().delete_all().await?;
            return Response::ok("deleted");
        }
        let stalled = self.with(|g| {
            g.ended_ms.is_none() && g.room.plies() < 2 && now >= g.last_activity_ms() + ABORT_AFTER_MS
        });
        let action = if stalled { Action::Abort { u: self.with(|g| g.meta.init.white.user_id) } } else { Action::Tick };
        let _ = self.act(action).await?;
        // `act` reschedules only after a change; make sure the alarm is set.
        self.schedule().await?;
        Response::ok("ok")
    }
}

impl GameObject {
    /// Loads the game from storage if it is not in memory. `false` if there is no game.
    async fn load(&self) -> Result<bool> {
        if self.game.borrow().is_some() {
            return Ok(true);
        }
        let Some(meta) = get_json::<Meta>(&self.state, "meta").await? else { return Ok(false) };
        let log: Vec<Logged> = get_json(&self.state, "log").await?.unwrap_or_default();
        let mut room = meta.init.clone().into_room(meta.created_ms);
        let mut ended_ms = None;
        for l in &log {
            let _ = room.apply(&l.a, l.t);
            if ended_ms.is_none() && room.is_over() {
                ended_ms = Some(l.t);
            }
        }
        let mut slot = self.game.borrow_mut();
        if slot.is_none() {
            *slot = Some(Loaded { meta, room, log, ended_ms });
        }
        Ok(true)
    }

    fn with<T>(&self, f: impl FnOnce(&mut Loaded) -> T) -> T {
        f(self.game.borrow_mut().as_mut().expect("game loaded"))
    }

    fn state_for(&self, viewer: Option<i64>) -> Value {
        self.with(|g| {
            let view = g.room.snapshot(viewer, now_ms());
            let positions = g.room.is_over().then(|| positions_json(g.room.game().history())).flatten();
            let mut v = state_json(&view, positions);
            hide_ratings(&mut v);
            v
        })
    }

    /// Applies an action; if it changed the room, logs it and tells everyone.
    async fn act(&self, action: Action) -> Result<std::result::Result<Vec<Event>, qchess_play::RoomError>> {
        let now = now_ms();
        let result = self.with(|g| g.room.apply(&action, now));
        let Ok(events) = &result else { return Ok(result) };
        if !changed(events) {
            return Ok(result);
        }
        let (log, ended_now, id) = self.with(|g| {
            g.log.push(Logged { t: now, a: action });
            let ended_now = g.ended_ms.is_none() && g.room.is_over();
            if ended_now {
                g.ended_ms = Some(now);
            }
            (serde_json::to_string(&g.log), ended_now, g.meta.init.game_id.clone())
        });
        put_json(&self.state, "log", log).await?;
        for ev in events {
            if let Event::Chat(line) = ev {
                let v = chat_json(line);
                for ws in self.state.get_websockets() {
                    send(&ws, &v);
                }
            }
        }
        if events.iter().any(|e| !matches!(e, Event::Chat(_) | Event::NeedsCollapseChoice { .. })) {
            for ws in self.state.get_websockets() {
                send(&ws, &self.state_for(who_of(&ws).id));
            }
        }
        self.schedule().await?;
        if ended_now {
            // Best effort: the lobby also forgets old games by itself.
            let lobby = self.env.durable_object("LOBBY").and_then(|ns| ns.get_by_name("lobby"));
            if let Ok(stub) = lobby {
                let _ = stub.fetch_with_str(&format!("https://lobby/ended?id={id}")).await;
            }
        }
        Ok(result)
    }

    async fn schedule(&self) -> Result<()> {
        let now = now_ms();
        let at = self.with(|g| g.next_alarm_ms(now));
        // A few ms late, so the flag has really fallen when the alarm runs.
        self.state.storage().set_alarm((at.saturating_sub(now) + 5) as i64).await
    }
}
