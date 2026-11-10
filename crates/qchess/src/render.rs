//! Text rendering of a quantum position.

use crate::quantum::Position;
use crate::types::*;

pub fn board_text(pos: &Position) -> String {
    let mut cells: Vec<Vec<(char, bool)>> = vec![Vec::new(); 64];
    for p in &pos.pieces {
        let split = p.is_split();
        for (i, h) in p.h.iter().enumerate() {
            if !h.alive || (i == 1 && !split) {
                continue;
            }
            let c = h.kind.fen_char(p.color);
            let cell = &mut cells[h.sq as usize];
            if !cell.contains(&(c, split)) {
                cell.push((c, split));
            }
        }
    }

    let mut out = String::new();
    out.push_str("   +-------------------------+\n");
    for r in (0..8u8).rev() {
        out.push_str(&format!(" {} |", r + 1));
        for f in 0..8u8 {
            let cell = &cells[sq(f, r) as usize];
            match cell.as_slice() {
                [] => out.push_str(" . "),
                [(c, q)] => out.push_str(&format!(" {}{}", c, if *q { '?' } else { ' ' })),
                [(c, _), ..] => out.push_str(&format!(" {c}*")),
            }
        }
        out.push_str(" |\n");
    }
    out.push_str("   +-------------------------+\n");
    out.push_str("     a  b  c  d  e  f  g  h\n");
    out.push_str(&format!("{} to move\n", pos.turn.name()));

    let split: Vec<_> = pos.pieces.iter().filter(|p| p.is_split()).collect();
    if !split.is_empty() {
        out.push_str("quantum pieces:\n");
        for p in split {
            let half = |i: usize| {
                let h = p.h[i];
                let k = if p.h[0].kind != p.h[1].kind {
                    format!("{} ", h.kind.name())
                } else {
                    String::new()
                };
                if h.alive {
                    format!("{k}{}", sq_name(h.sq))
                } else {
                    format!("{k}captured on {}", sq_name(h.sq))
                }
            };
            let kind = if p.h[0].kind == p.h[1].kind {
                p.h[0].kind.name()
            } else {
                "piece"
            };
            out.push_str(&format!("  {} {}: {} | {}\n", p.color.name(), kind, half(0), half(1)));
        }
    }
    out
}
