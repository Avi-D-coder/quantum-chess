//! Shared application state: the lobby, live game rooms and their broadcast
//! channels. Everything here is synchronous; locks are never held across an
//! await point.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

use glicko2::Rating;
use qchess_play::{
    ColorPref, Event, Lobby, LobbyError, PlayerInfo, Room, RoomError, RoomInit, Seek, TimeControl,
};
use rand::Rng;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use qchess_play::{bot::Level, json};
use crate::db::{Db, MoveRow, User, now_ms};

/// Seconds without a move after which an unstarted game is aborted.
const ABORT_AFTER_MS: u64 = 30_000;
/// A seek disappears when its owner has not polled the lobby for this long.
const SEEK_IDLE_MS: u64 = 25_000;
/// How long a finished room stays in memory.
const KEEP_ENDED_MS: u64 = 5 * 60_000;

#[derive(Clone)]
pub enum Bc {
    /// The room changed; every socket re-renders its own snapshot.
    State,
    Chat(Value),
}

pub struct Live {
    pub room: Mutex<Room>,
    pub tx: broadcast::Sender<Bc>,
    ended_ms: Mutex<Option<u64>>,
    /// The computer player in this game, if any.
    pub bot: Option<BotSeat>,
}

pub struct BotSeat {
    pub user_id: i64,
    pub level: Level,
    /// A search for this game is running.
    thinking: AtomicBool,
}

impl Live {
    pub fn room(&self) -> MutexGuard<'_, Room> {
        self.room.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Default)]
pub struct Hub {
    lobby: Lobby,
    rooms: HashMap<String, Arc<Live>>,
    /// user id -> game started from one of their seeks, until they poll.
    matched: HashMap<i64, String>,
    /// user id -> last lobby poll (epoch ms).
    seen: HashMap<i64, u64>,
}

pub struct PendingOAuth {
    pub provider: String,
    pub verifier: String,
    pub next: String,
    pub created_ms: u64,
}

pub struct Config {
    pub public_url: String,
    pub env: HashMap<String, String>,
}

impl Config {
    pub fn from_env() -> Config {
        let env: HashMap<String, String> = std::env::vars().collect();
        let port = env.get("PORT").cloned().unwrap_or_else(|| "3000".into());
        let public_url = env
            .get("PUBLIC_URL")
            .cloned()
            .unwrap_or_else(|| format!("http://localhost:{port}"))
            .trim_end_matches('/')
            .to_string();
        Config { public_url, env }
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.env.get(k).map(String::as_str).filter(|s| !s.is_empty())
    }
}

pub struct App {
    pub db: Db,
    pub cfg: Config,
    pub http: reqwest::Client,
    hub: Mutex<Hub>,
    pub oauth: Mutex<HashMap<String, PendingOAuth>>,
}

#[derive(Debug)]
pub enum ActionError {
    Lobby(LobbyError),
    BadRequest(&'static str),
    Db,
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::Lobby(e) => write!(f, "{e}"),
            ActionError::BadRequest(s) => f.write_str(s),
            ActionError::Db => f.write_str("database error"),
        }
    }
}

pub fn random_id(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    (0..len).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect()
}

impl App {
    pub fn new(db: Db, cfg: Config) -> App {
        App {
            db,
            cfg,
            http: reqwest::Client::builder().user_agent("quantum-chess").build().expect("http client"),
            hub: Mutex::new(Hub::default()),
            oauth: Mutex::new(HashMap::new()),
        }
    }

    fn hub(&self) -> MutexGuard<'_, Hub> {
        self.hub.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn player_info(&self, user: &User, tc: TimeControl) -> PlayerInfo {
        let (r, _) = self.db.rating(user.id, tc.speed().name());
        PlayerInfo { user_id: user.id, username: user.name().to_string(), rating: Some(r) }
    }

    // ------------------------------------------------------------ lobby

    pub fn create_seek(
        &self,
        user: &User,
        tc_label: &str,
        rated: bool,
        color: &str,
    ) -> Result<(), ActionError> {
        let tc = TimeControl::from_label(tc_label).ok_or(ActionError::BadRequest("unknown time control"))?;
        let color = ColorPref::parse(color).ok_or(ActionError::BadRequest("unknown color"))?;
        let (r, _) = self.db.rating(user.id, tc.speed().name());
        let seek = Seek {
            id: random_id(8),
            user_id: user.id,
            username: user.name().to_string(),
            rating: Some(r.rating),
            player_rating: Some(r),
            tc,
            rated,
            color,
            created_ms: now_ms(),
        };
        let mut hub = self.hub();
        hub.seen.insert(user.id, now_ms());
        hub.lobby.add_seek(seek);
        Ok(())
    }

    pub fn cancel_seek(&self, user: &User, id: &str) -> Result<(), ActionError> {
        self.hub().lobby.cancel_seek(user.id, id).map(|_| ()).map_err(ActionError::Lobby)
    }

    pub fn accept_seek(&self, user: &User, id: &str) -> Result<String, ActionError> {
        let seek_owner = self.hub().lobby.get_seek(id).map(|s| s.user_id);
        let tc = self.hub().lobby.get_seek(id).map(|s| s.tc);
        let me = tc.map(|tc| self.player_info(user, tc)).ok_or(ActionError::Lobby(LobbyError::NotFound))?;
        let init = {
            let mut hub = self.hub();
            hub.lobby
                .accept_seek(me, id, random_id(8), rand::rng().random_bool(0.5))
                .map_err(ActionError::Lobby)?
        };
        let game_id = self.start_game(init)?;
        if let Some(owner) = seek_owner {
            self.hub().matched.insert(owner, game_id.clone());
        }
        Ok(game_id)
    }

    pub fn create_challenge(&self, user: &User, tc_label: &str, rated: bool, color: &str) -> Result<String, ActionError> {
        let tc = TimeControl::from_label(tc_label).ok_or(ActionError::BadRequest("unknown time control"))?;
        let color = ColorPref::parse(color).ok_or(ActionError::BadRequest("unknown color"))?;
        let token = random_id(12);
        let creator = self.player_info(user, tc);
        self.hub().lobby.create_challenge(creator, tc, rated, color, token.clone(), now_ms());
        Ok(token)
    }

    pub fn challenge_info(&self, token: &str) -> Option<(String, TimeControl, bool, i64)> {
        let hub = self.hub();
        let c = hub.lobby.get_challenge(token)?;
        Some((c.creator.username.clone(), c.tc, c.rated, c.creator.user_id))
    }

    pub fn accept_challenge(&self, user: &User, token: &str) -> Result<String, ActionError> {
        let tc = self.challenge_info(token).map(|c| c.1).ok_or(ActionError::Lobby(LobbyError::NotFound))?;
        let me = self.player_info(user, tc);
        let init = self
            .hub()
            .lobby
            .accept_challenge(me, token, random_id(8), rand::rng().random_bool(0.5), now_ms())
            .map_err(ActionError::Lobby)?;
        let creator = if init.white.user_id == user.id { init.black.user_id } else { init.white.user_id };
        let game_id = self.start_game(init)?;
        self.hub().matched.insert(creator, game_id.clone());
        Ok(game_id)
    }

    pub fn cancel_challenge(&self, user: &User, token: &str) -> Result<(), ActionError> {
        self.hub().lobby.cancel_challenge(user.id, token).map(|_| ()).map_err(ActionError::Lobby)
    }

    /// Lobby snapshot for `user` (marks them as present, consumes a match).
    pub fn lobby_json(&self, user: Option<&User>) -> Value {
        let now = now_ms();
        let mut hub = self.hub();
        let mut matched = None;
        if let Some(u) = user {
            hub.seen.insert(u.id, now);
            matched = hub.matched.remove(&u.id);
        }
        let seeks: Vec<Value> = hub
            .lobby
            .list()
            .into_iter()
            .map(|s| {
                json!({
                    "id": s.id, "user": s.username, "rating": s.rating.map(|r| r.round() as i64),
                    "tc": s.tc.label(), "speed": s.tc.speed().name(), "rated": s.rated,
                    "color": s.color.as_str(), "mine": user.is_some_and(|u| u.id == s.user_id),
                })
            })
            .collect();
        let mut games: Vec<(i64, Value)> = hub
            .rooms
            .values()
            .filter_map(|l| {
                let r = l.room();
                if r.is_over() {
                    return None;
                }
                let avg = avg_rating(&r);
                Some((
                    avg,
                    json!({
                        "id": r.id(), "white": r.white().username, "black": r.black().username,
                        "white_rating": rating_of(r.white()), "black_rating": rating_of(r.black()),
                        "tc": r.time_control().label(), "rated": r.rated(),
                    }),
                ))
            })
            .collect();
        games.sort_by_key(|g| std::cmp::Reverse(g.0));
        json!({
            "seeks": seeks,
            "games": games.into_iter().map(|g| g.1).take(20).collect::<Vec<_>>(),
            "matched": matched,
            "tcs": qchess_play::PRESETS.iter().map(|t| t.label()).collect::<Vec<_>>(),
        })
    }

    /// Id of the highest-rated ongoing game.
    pub fn tv_game(&self) -> Option<String> {
        let hub = self.hub();
        hub.rooms
            .values()
            .filter_map(|l| {
                let r = l.room();
                (!r.is_over()).then(|| (avg_rating(&r), r.id().to_string()))
            })
            .max()
            .map(|(_, id)| id)
    }

    // ------------------------------------------------------------ games

    /// A casual game against the computer at `level`.
    pub fn start_bot_game(&self, user: &User, level: &str, tc_label: &str, color: &str) -> Result<String, ActionError> {
        let level = Level::parse(level).ok_or(ActionError::BadRequest("unknown level"))?;
        let tc = TimeControl::from_label(tc_label).ok_or(ActionError::BadRequest("unknown time control"))?;
        let color = ColorPref::parse(color).ok_or(ActionError::BadRequest("unknown color"))?;
        let bot = self.db.bot_user(level.key(), level.username()).map_err(|_| ActionError::Db)?;
        let me = self.player_info(user, tc);
        let them = self.player_info(&bot, tc);
        let human_white = match color {
            ColorPref::White => true,
            ColorPref::Black => false,
            ColorPref::Random => rand::rng().random_bool(0.5),
        };
        let (white, black) = if human_white { (me, them) } else { (them, me) };
        let init = RoomInit { game_id: random_id(8), white, black, tc, rated: false };
        self.start_live(init, Some(BotSeat { user_id: bot.id, level, thinking: AtomicBool::new(false) }))
    }

    fn start_game(&self, init: RoomInit) -> Result<String, ActionError> {
        self.start_live(init, None)
    }

    fn start_live(&self, init: RoomInit, bot: Option<BotSeat>) -> Result<String, ActionError> {
        let id = init.game_id.clone();
        self.db
            .insert_game(&id, init.white.user_id, init.black.user_id, init.tc.initial_ms, init.tc.increment_ms, init.rated)
            .map_err(|_| ActionError::Db)?;
        let room = init.into_room(now_ms());
        let (tx, _) = broadcast::channel(64);
        let live = Arc::new(Live { room: Mutex::new(room), tx, ended_ms: Mutex::new(None), bot });
        self.hub().rooms.insert(id.clone(), live);
        Ok(id)
    }

    pub fn live(&self, id: &str) -> Option<Arc<Live>> {
        self.hub().rooms.get(id).cloned()
    }

    /// Applies room events: persists moves and results, updates ratings and
    /// notifies sockets. Returns the collapse-choice request, if any.
    pub fn apply_events(&self, live: &Live, events: Vec<Event>) -> Option<Value> {
        let mut choice = None;
        let mut changed = false;
        for ev in events {
            match ev {
                Event::Moved(m) => {
                    let room = live.room();
                    self.db.insert_move(
                        room.id(),
                        &MoveRow {
                            ply: m.ply,
                            notation: m.notation.clone(),
                            collapses: m.collapses.clone(),
                            white_ms: m.white_ms,
                            black_ms: m.black_ms,
                        },
                        m.at_ms,
                    );
                    changed = true;
                }
                Event::Ended { result, termination } => {
                    let (id, updates, tc, white, black) = {
                        let room = live.room();
                        (
                            room.id().to_string(),
                            room.rating_updates(),
                            room.time_control(),
                            room.white().user_id,
                            room.black().user_id,
                        )
                    };
                    self.db.finish_game(&id, result.as_str(), termination.as_str());
                    if let Some((w, b)) = updates {
                        let speed = tc.speed().name();
                        for (uid, r) in [(white, w), (black, b)] {
                            let (_, n) = self.db.rating(uid, speed);
                            self.db.set_rating(uid, speed, r, n + 1);
                        }
                    }
                    *live.ended_ms.lock().unwrap_or_else(|e| e.into_inner()) = Some(now_ms());
                    changed = true;
                }
                Event::Chat(line) => {
                    let _ = live.tx.send(Bc::Chat(chat_json(&line)));
                }
                Event::DrawOffered(_) | Event::DrawDeclined => changed = true,
                Event::NeedsCollapseChoice { notation, options } => {
                    choice = Some(json!({ "type": "choose_collapse", "notation": notation, "options": options }));
                }
            }
        }
        if changed {
            let _ = live.tx.send(Bc::State);
        }
        choice
    }

    /// A client message from a socket. Returns an optional direct reply.
    pub fn client_message(&self, live: &Live, user: Option<&User>, msg: &Value) -> Option<Value> {
        let err = |m: String| Some(json!({ "type": "error", "message": m }));
        let Some(user) = user else { return err("sign in to play".into()) };
        let now = now_ms();
        let kind = msg.get("type").and_then(Value::as_str).unwrap_or("");
        let result: Result<Vec<Event>, RoomError> = {
            let mut room = live.room();
            match kind {
                "move" => {
                    let n = msg.get("notation").and_then(Value::as_str).unwrap_or("");
                    room.play(user.id, n, now)
                }
                "resign" => room.resign(user.id, now),
                "abort" => room.abort(user.id, now),
                "draw_offer" => room.offer_draw(user.id),
                "draw_accept" => room.accept_draw(user.id, now),
                "draw_decline" => room.decline_draw(user.id),
                "chat" => {
                    let t = msg.get("text").and_then(Value::as_str).unwrap_or("");
                    room.chat(user.id, user.name(), t, now)
                }
                _ => return err("unknown message".into()),
            }
        };
        match result {
            Ok(events) => self.apply_events(live, events),
            Err(e) => err(e.to_string()),
        }
    }

    /// State message for `viewer`, with replay positions once finished.
    pub fn snapshot_json(&self, live: &Live, viewer: Option<i64>) -> Value {
        let room = live.room();
        let view = room.snapshot(viewer, now_ms());
        let positions = room.is_over().then(|| json::positions_json(room.game().history())).flatten();
        json::state_json(&view, positions)
    }

    pub fn chat_history(&self, live: &Live) -> Vec<Value> {
        live.room().chat_log().iter().map(chat_json).collect()
    }

    /// Periodic housekeeping: flag timeouts, abort stalled starts, drop idle
    /// seeks and old rooms.
    pub fn tick(self: &Arc<Self>) {
        let now = now_ms();
        let rooms: Vec<Arc<Live>> = self.hub().rooms.values().cloned().collect();
        for live in rooms {
            self.bot_turn(&live, now);
            let events = {
                let mut room = live.room();
                let mut ev = room.tick(now);
                if ev.is_empty() && !room.is_over() && room.plies() < 2 && now.saturating_sub(room.moves().last().map_or(room.created_ms(), |m| m.at_ms)) > ABORT_AFTER_MS {
                    let uid = room.white().user_id;
                    ev = room.abort(uid, now).unwrap_or_default();
                }
                ev
            };
            if !events.is_empty() {
                self.apply_events(&live, events);
            }
        }
        let mut hub = self.hub();
        hub.rooms.retain(|_, l| {
            let ended = *l.ended_ms.lock().unwrap_or_else(|e| e.into_inner());
            ended.is_none_or(|t| now.saturating_sub(t) < KEEP_ENDED_MS)
        });
        let idle: Vec<i64> = hub
            .seen
            .iter()
            .filter(|(_, t)| now.saturating_sub(**t) > SEEK_IDLE_MS)
            .map(|(&u, _)| u)
            .collect();
        for u in idle {
            hub.lobby.remove_user_seeks(u);
            hub.seen.remove(&u);
        }
        hub.lobby.prune(now);
        drop(hub);
        self.oauth
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, p| now.saturating_sub(p.created_ms) < 10 * 60_000);
    }
}

impl App {
    /// Starts the computer's search when it is to move, and declines draw
    /// offers made to it. The search runs on a blocking thread; its move is
    /// played through the room like any player's.
    fn bot_turn(self: &Arc<Self>, live: &Arc<Live>, now: u64) {
        let Some(bot) = &live.bot else { return };
        let (game, remaining, increment) = {
            let mut room = live.room();
            if room.is_over() {
                return;
            }
            let bot_color = room.color_of(bot.user_id);
            if room.draw_offer().is_some_and(|c| Some(c) != bot_color)
                && let Ok(ev) = room.decline_draw(bot.user_id)
            {
                drop(room);
                self.apply_events(live, ev);
                room = live.room();
            }
            let to_move = room.game().to_move();
            if bot_color != Some(to_move) || bot.thinking.swap(true, Ordering::SeqCst) {
                return;
            }
            (room.game().clone(), room.clock().remaining(to_move, now), room.time_control().increment_ms)
        };
        let cfg = bot.level.config(remaining, increment);
        let seed: u64 = rand::rng().random();
        let (app, live) = (self.clone(), live.clone());
        tokio::spawn(async move {
            let plies = game.history().len();
            let choice =
                tokio::task::spawn_blocking(move || qchess::ai::Searcher::new(cfg).seed(seed).choose(&game)).await;
            let bot = live.bot.as_ref().expect("bot game");
            if let Ok(Some(c)) = choice {
                let events = {
                    let mut room = live.room();
                    // Skip if the game moved on meanwhile (ended, aborted).
                    if room.game().history().len() == plies {
                        room.play(bot.user_id, &c.notation, now_ms()).unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                };
                app.apply_events(&live, events);
            }
            bot.thinking.store(false, Ordering::SeqCst);
        });
    }
}

fn rating_of(p: &PlayerInfo) -> i64 {
    p.rating.unwrap_or_default().rating.round() as i64
}

fn avg_rating(r: &Room) -> i64 {
    (rating_of(r.white()) + rating_of(r.black())) / 2
}

pub use qchess_play::json::chat_json;

/// Ratings are stored per speed; used by pages.
pub fn rating_display(r: Rating) -> i64 {
    r.rating.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Arc<App> {
        let db = Db::open(":memory:").unwrap();
        Arc::new(App::new(db, Config { public_url: "http://localhost".into(), env: HashMap::new() }))
    }

    fn human(a: &App) -> User {
        let u = a.db.upsert_identity("test", "1", "Ann", None, None).unwrap();
        a.db.set_username(u.id, "ann").unwrap();
        a.db.user(u.id).unwrap()
    }

    async fn wait_for_plies(a: &Arc<App>, live: &Live, n: usize) {
        for _ in 0..200 {
            a.tick();
            if live.room().plies() >= n {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the bot did not move");
    }

    #[tokio::test]
    async fn the_bot_replies_and_reuses_its_account() {
        let a = app();
        let me = human(&a);
        // Human is black: the bot opens.
        let id = a.start_bot_game(&me, "easy", "5+0", "black").unwrap();
        let live = a.live(&id).unwrap();
        assert!(!live.room().rated());
        assert_eq!(live.room().white().username, "QBot-Easy");
        wait_for_plies(&a, &live, 1).await;
        // Our reply, then the bot's.
        let (from, to) = {
            let room = live.room();
            let snap = room.snapshot(Some(me.id), now_ms());
            let (f, t) = snap.targets.first().cloned().unwrap();
            (f, t[0].clone())
        };
        let r = a.client_message(&live, Some(&me), &json!({ "type": "move", "notation": format!("{from}{to}") }));
        assert!(r.is_none(), "{r:?}");
        wait_for_plies(&a, &live, 3).await;
        // A second game uses the same bot account.
        let id2 = a.start_bot_game(&me, "easy", "5+0", "white").unwrap();
        assert_eq!(a.live(&id2).unwrap().room().black().user_id, live.room().white().user_id);
        assert!(a.start_bot_game(&me, "grandmaster", "5+0", "white").is_err());
    }

    #[tokio::test]
    async fn the_bot_declines_draw_offers() {
        let a = app();
        let me = human(&a);
        let id = a.start_bot_game(&me, "easy", "5+0", "white").unwrap();
        let live = a.live(&id).unwrap();
        a.client_message(&live, Some(&me), &json!({ "type": "move", "notation": "e2e4" }));
        wait_for_plies(&a, &live, 2).await;
        a.client_message(&live, Some(&me), &json!({ "type": "draw_offer" }));
        assert!(live.room().draw_offer().is_some());
        a.tick();
        assert!(live.room().draw_offer().is_none());
    }
}
