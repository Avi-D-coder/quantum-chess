//! Wire format of docs/PROTOCOL.md.

use qchess::{Game, PieceView};
use qchess_play::{ClockView, MoveRecord, StateView, StatusView, color_str};
use serde_json::{Value, json};

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
