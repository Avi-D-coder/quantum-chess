//! Quantum chess against the computer, entirely in the browser.
//!
//! [`LocalGame`] is a [`qchess_play::Room`] between the browser's player and
//! the computer, speaking the same JSON as the server (docs/PROTOCOL.md), so
//! the board UI cannot tell it from a live game. It is meant to run in a Web
//! Worker (`site/local-worker.js`), since the computer's search blocks for up
//! to a few seconds. The game is saved as its settings plus action log
//! ([`LocalGame::save`]), so a reload can pick it up again.

use qchess_play::{
    Action, Logged, PlayerInfo, Room, TimeControl,
    action::changed,
    bot::Level,
    json::{chat_json, client_action, collapse_choice, error_json, hide_ratings, positions_json, state_json},
};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

const HUMAN: i64 = 1;
const BOT: i64 = 2;

/// A game against the computer. See the crate docs.
#[wasm_bindgen]
pub struct LocalGame {
    settings: Value,
    level: Level,
    room: Room,
    log: Vec<Logged>,
}

fn room_for(settings: &Value) -> Result<(Room, Level), JsError> {
    let s = |k: &str| settings.get(k).and_then(Value::as_str).unwrap_or("");
    let tc = TimeControl::from_label(s("tc")).ok_or_else(|| JsError::new("unknown time control"))?;
    let level = Level::parse(s("level")).ok_or_else(|| JsError::new("unknown level"))?;
    let name = if s("name").is_empty() { "You" } else { s("name") };
    let human = PlayerInfo { user_id: HUMAN, username: name.to_string(), rating: None };
    let bot = PlayerInfo { user_id: BOT, username: level.username().to_string(), rating: None };
    let human_white = settings.get("human_white").and_then(Value::as_bool).unwrap_or(true);
    let (white, black) = if human_white { (human, bot) } else { (bot, human) };
    let created = settings.get("created_ms").and_then(Value::as_u64).unwrap_or(0);
    Ok((Room::new("local", white, black, tc, false, created), level))
}

#[wasm_bindgen]
impl LocalGame {
    /// A new game: time control label (`"5+3"`), `"white"`/`"black"`, level
    /// (`"easy"`/`"medium"`/`"hard"`), the player's name and the current time.
    #[wasm_bindgen(constructor)]
    pub fn new(tc: &str, human_white: bool, level: &str, name: &str, now_ms: f64) -> Result<LocalGame, JsError> {
        let settings = json!({ "tc": tc, "human_white": human_white, "level": level, "name": name,
                               "created_ms": now_ms as u64 });
        let (room, level) = room_for(&settings)?;
        Ok(LocalGame { settings, level, room, log: Vec::new() })
    }

    /// A game saved with [`save`](Self::save).
    pub fn restore(saved: &str) -> Result<LocalGame, JsError> {
        let v: Value = serde_json::from_str(saved).map_err(|e| JsError::new(&e.to_string()))?;
        let settings = v.get("settings").cloned().unwrap_or(Value::Null);
        let log: Vec<Logged> = serde_json::from_value(v.get("log").cloned().unwrap_or(Value::Null))
            .map_err(|e| JsError::new(&e.to_string()))?;
        let (mut room, level) = room_for(&settings)?;
        room.replay(&log);
        Ok(LocalGame { settings, level, room, log })
    }

    /// Settings and action log, as JSON.
    pub fn save(&self) -> String {
        json!({ "settings": self.settings, "log": self.log }).to_string()
    }

    /// Whether the game has ended.
    pub fn over(&self) -> bool {
        self.room.is_over()
    }

    /// The messages to send on connect: the state and the chat so far (a JSON array).
    pub fn hello(&self, now_ms: f64) -> String {
        let mut out = vec![self.state(now_ms as u64)];
        out.extend(self.room.chat_log().iter().map(chat_json));
        Value::Array(out).to_string()
    }

    /// Handles a client message (JSON); returns the replies (a JSON array).
    pub fn message(&mut self, msg: &str, now_ms: f64) -> String {
        let now = now_ms as u64;
        let Ok(v) = serde_json::from_str::<Value>(msg) else { return "[]".into() };
        let name = self.room.player(self.human_color()).username.clone();
        let mut out = match client_action(&v, HUMAN, &name) {
            Ok(a) => self.act(a, now),
            Err(e) => vec![error_json(e)],
        };
        // The computer never takes draws.
        if self.room.draw_offer() == Some(self.human_color()) {
            out.extend(self.act(Action::DeclineDraw { u: BOT }, now));
        }
        Value::Array(out).to_string()
    }

    /// Flags a player whose time ran out; returns any messages (a JSON array).
    pub fn tick(&mut self, now_ms: f64) -> String {
        Value::Array(self.act(Action::Tick, now_ms as u64)).to_string()
    }

    /// When [`tick`](Self::tick) should next be called (epoch ms), or -1.
    pub fn next_deadline(&self, now_ms: f64) -> f64 {
        self.room.next_deadline_ms(now_ms as u64).map_or(-1.0, |t| t as f64)
    }

    /// Whether it is the computer's move.
    pub fn bot_to_move(&self) -> bool {
        !self.room.is_over() && self.room.game().to_move() != self.human_color()
    }

    /// Runs the computer's search (blocking) and returns its move, if any.
    pub fn think(&self, now_ms: f64, seed: f64) -> Option<String> {
        let side = self.room.game().to_move();
        let remaining = self.room.clock().remaining(side, now_ms as u64);
        let cfg = self.level.config(remaining, self.room.time_control().increment_ms);
        qchess::ai::Searcher::new(cfg).seed(seed as u64).choose(self.room.game()).map(|c| c.notation)
    }

    /// Plays the computer's move found by [`think`](Self::think); returns the messages.
    pub fn play_bot(&mut self, notation: &str, now_ms: f64) -> String {
        Value::Array(self.act(Action::Move { u: BOT, n: notation.to_string() }, now_ms as u64)).to_string()
    }

    /// The start position and the position after every move (a JSON array).
    pub fn positions(&self) -> String {
        positions_json(self.room.game().history()).unwrap_or(Value::Array(Vec::new())).to_string()
    }
}

impl LocalGame {
    fn human_color(&self) -> qchess::Color {
        self.room.color_of(HUMAN).expect("human plays")
    }

    fn state(&self, now: u64) -> Value {
        let view = self.room.snapshot(Some(HUMAN), now);
        let positions = self.room.is_over().then(|| positions_json(self.room.game().history())).flatten();
        let mut v = state_json(&view, positions);
        hide_ratings(&mut v);
        v
    }

    fn act(&mut self, action: Action, now: u64) -> Vec<Value> {
        match self.room.apply(&action, now) {
            Err(e) => vec![error_json(e)],
            Ok(events) => {
                let mut out = Vec::new();
                if let Some(c) = collapse_choice(&events) {
                    out.push(c);
                }
                if changed(&events) {
                    self.log.push(Logged { t: now, a: action });
                    for e in &events {
                        if let qchess_play::Event::Chat(line) = e {
                            out.push(chat_json(line));
                        }
                    }
                    out.push(self.state(now));
                }
                out
            }
        }
    }
}
