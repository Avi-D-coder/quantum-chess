//! Wire format of docs/PROTOCOL.md (feature `json`).

use qchess::{Game, PieceView};
use crate::{Action, ChatLine, Event, ClockView, MoveRecord, StateView, StatusView, color_str};
use serde_json::{Value, json};

/// JSON for the protocol (see docs/PROTOCOL.md).
pub fn pieces_json(pieces: &[PieceView]) -> Value {
    pieces
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "color": color_str(p.color),
                "origin": p.origin.name(),
                "num": p.num,
                "split": p.split,
                "halves": p.halves.iter().map(|h| json!({
                    "square": h.square, "kind": h.kind.name(), "alive": h.alive,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// JSON for the protocol (see docs/PROTOCOL.md).
pub fn clock_json(c: &ClockView) -> Value {
    json!({
        "initial_ms": c.initial_ms,
        "increment_ms": c.increment_ms,
        "white_ms": c.white_ms,
        "black_ms": c.black_ms,
        "running": c.running.map(color_str),
        "server_now_ms": c.server_now_ms,
    })
}

/// JSON for the protocol (see docs/PROTOCOL.md).
pub fn move_json(m: &MoveRecord) -> Value {
    json!({
        "ply": m.ply, "notation": m.notation, "display": m.display, "collapses": m.collapses,
        "white_ms": m.white_ms, "black_ms": m.black_ms,
    })
}

/// The `state` message. `positions` (start position plus one per ply) is
/// included for finished games so the client can step through them.
pub fn state_json(s: &StateView, positions: Option<Value>) -> Value {
    let status = match s.status {
        StatusView::Ongoing => json!({ "kind": "ongoing" }),
        StatusView::Ended { result, termination } => {
            json!({ "kind": "ended", "result": result, "termination": termination })
        }
    };
    let targets: serde_json::Map<String, Value> =
        s.targets.iter().map(|(from, to)| (from.clone(), json!(to))).collect();
    let mut v = json!({
        "type": "state",
        "id": s.id,
        "you": s.you_str(),
        "players": {
            "white": { "username": s.white.username, "rating": s.white.rating, "provisional": s.white.provisional },
            "black": { "username": s.black.username, "rating": s.black.rating, "provisional": s.black.provisional },
        },
        "clock": clock_json(&s.clock),
        "to_move": color_str(s.to_move),
        "status": status,
        "check": s.check_str(),
        "state_count": s.state_count,
        "pieces": pieces_json(&s.pieces),
        "targets": targets,
        "last_move": s.last_move,
        "moves": s.moves.iter().map(move_json).collect::<Vec<_>>(),
        "draw_offer": s.draw_offer.map(color_str),
        "rated": s.rated,
    });
    if let Some(p) = positions {
        v["positions"] = p;
    }
    v
}

/// Start position and the position after every move of `history`.
/// `None` if the history does not replay.
pub fn positions_json(history: &[String]) -> Option<Value> {
    let games = Game::replay(history).ok()?;
    Some(
        games
            .iter()
            .map(|g| json!({ "pieces": pieces_json(&g.pieces()), "last_move": g.last_move_squares() }))
            .collect(),
    )
}

/// A `chat` message.
pub fn chat_json(l: &ChatLine) -> Value {
    json!({ "type": "chat", "user": l.username, "text": l.text, "ts": l.ts })
}

/// Parses a client message of docs/PROTOCOL.md into an action by player
/// `user` (shown as `name`). The error is the text of an `error` message.
pub fn client_action(msg: &Value, user: i64, name: &str) -> Result<Action, String> {
    let str_field = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(match msg.get("type").and_then(Value::as_str).unwrap_or("") {
        "move" => Action::Move { u: user, n: str_field("notation") },
        "resign" => Action::Resign { u: user },
        "abort" => Action::Abort { u: user },
        "draw_offer" => Action::OfferDraw { u: user },
        "draw_accept" => Action::AcceptDraw { u: user },
        "draw_decline" => Action::DeclineDraw { u: user },
        "chat" => Action::Chat { u: user, name: name.to_string(), text: str_field("text") },
        _ => return Err("unknown message".into()),
    })
}

/// The `choose_collapse` message among `events`, if a move needs a choice.
pub fn collapse_choice(events: &[Event]) -> Option<Value> {
    events.iter().find_map(|e| match e {
        Event::NeedsCollapseChoice { notation, options } => {
            Some(json!({ "type": "choose_collapse", "notation": notation, "options": options }))
        }
        _ => None,
    })
}

/// Removes the ratings from a `state` message, for sites without ratings
/// (the board then shows names only).
pub fn hide_ratings(state: &mut Value) {
    for side in ["white", "black"] {
        if let Some(p) = state.get_mut("players").and_then(|p| p.get_mut(side)) {
            p["rating"] = Value::Null;
        }
    }
}

/// An `error` message.
pub fn error_json(message: impl std::fmt::Display) -> Value {
    json!({ "type": "error", "message": message.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_have_one_more_entry_than_moves() {
        let h = vec!["e2e4".to_string(), "e7e5".to_string()];
        let p = positions_json(&h).unwrap();
        assert_eq!(p.as_array().unwrap().len(), 3);
        assert!(positions_json(&["zz".to_string()]).is_none());
    }
}
