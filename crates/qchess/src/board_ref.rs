//! The original square-scanning board, kept only so tests can check the
//! bitboard board in `board.rs` against it.
//!
//! A single classical board state and ordinary chess move rules.
//!
//! Quantum positions are sets of these; every legality question in the
//! quantum rules ("would this be legal in regular chess from that state?")
//! is answered here.
#![allow(dead_code)]

use crate::types::*;

pub use crate::board::Occ;

pub const KING_SIDE: usize = 0;
pub const QUEEN_SIDE: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    Normal,
    DoublePush,
    EnPassant,
    Castle(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CMove {
    pub from: Sq,
    pub to: Sq,
    pub promo: Option<Kind>,
    pub kind: MoveKind,
}

#[derive(Clone, Debug)]
pub struct Board {
    pub sq: [Option<Occ>; 64],
    pub turn: Color,
    pub ep: Option<Sq>,
    /// `castle[color][side]`
    pub castle: [[bool; 2]; 2],
    /// Where each king stands (indexed like `castle`); 64 if it is off the
    /// board. Kept in step with `sq` so check tests needn't search for it.
    pub kings: [Sq; 2],
}

const KNIGHT: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
const KING: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
const DIAG: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const ORTH: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

fn pawn_dir(c: Color) -> i8 {
    match c {
        Color::White => 1,
        Color::Black => -1,
    }
}

impl Board {
    pub fn empty(turn: Color) -> Self {
        Board {
            sq: [None; 64],
            turn,
            ep: None,
            castle: [[false; 2]; 2],
            kings: [64; 2],
        }
    }

    /// Put a piece on an empty square, keeping the king squares current.
    pub fn place(&mut self, s: Sq, o: Occ) {
        if o.kind == Kind::King {
            self.kings[o.color.idx()] = s;
        }
        self.sq[s as usize] = Some(o);
    }

    fn is(&self, s: Sq, c: Color, k: Kind) -> bool {
        matches!(self.sq[s as usize], Some(o) if o.color == c && o.kind == k)
    }

    pub fn king_sq(&self, c: Color) -> Option<Sq> {
        (0..64u8).find(|&s| self.is(s, c, Kind::King))
    }

    /// Is square `s` attacked by any piece of color `by`?
    pub fn attacked(&self, s: Sq, by: Color) -> bool {
        // A pawn of `by` attacking `s` sits one rank "behind" it from its view.
        let dr = -pawn_dir(by);
        for df in [-1, 1] {
            if let Some(p) = offset(s, df, dr)
                && self.is(p, by, Kind::Pawn) {
                    return true;
                }
        }
        for (df, dr) in KNIGHT {
            if let Some(p) = offset(s, df, dr)
                && self.is(p, by, Kind::Knight) {
                    return true;
                }
        }
        for (df, dr) in KING {
            if let Some(p) = offset(s, df, dr)
                && self.is(p, by, Kind::King) {
                    return true;
                }
        }
        for (dirs, a, b) in [
            (DIAG, Kind::Bishop, Kind::Queen),
            (ORTH, Kind::Rook, Kind::Queen),
        ] {
            for (df, dr) in dirs {
                let mut cur = s;
                while let Some(p) = offset(cur, df, dr) {
                    if let Some(o) = self.sq[p as usize] {
                        if o.color == by && (o.kind == a || o.kind == b) {
                            return true;
                        }
                        break;
                    }
                    cur = p;
                }
            }
        }
        false
    }

    pub fn in_check(&self, c: Color) -> bool {
        self.king_sq(c)
            .is_some_and(|k| self.attacked(k, c.other()))
    }

    /// Pseudo-legal moves (own king safety not checked, except castling
    /// rules about check) for the piece of the side to move on `from`, handed
    /// one by one to `out` so callers needn't allocate.
    pub fn each_pseudo(&self, from: Sq, out: &mut impl FnMut(CMove)) {
        let Some(o) = self.sq[from as usize] else {
            return;
        };
        if o.color != self.turn {
            return;
        }
        let c = o.color;
        let mut push = |to: Sq, kind: MoveKind, promo: Option<Kind>| {
            out(CMove {
                from,
                to,
                promo,
                kind,
            })
        };
        match o.kind {
            Kind::Pawn => {
                let dir = pawn_dir(c);
                let start = if c == Color::White { 1 } else { 6 };
                let last = if c == Color::White { 7 } else { 0 };
                let mut pawn_to = |to: Sq, kind: MoveKind| {
                    if rank_of(to) == last {
                        for k in [Kind::Queen, Kind::Rook, Kind::Bishop, Kind::Knight] {
                            push(to, kind, Some(k));
                        }
                    } else {
                        push(to, kind, None);
                    }
                };
                if let Some(t) = offset(from, 0, dir)
                    && self.sq[t as usize].is_none() {
                        pawn_to(t, MoveKind::Normal);
                        if rank_of(from) == start
                            && let Some(t2) = offset(from, 0, 2 * dir)
                                && self.sq[t2 as usize].is_none() {
                                    pawn_to(t2, MoveKind::DoublePush);
                                }
                    }
                for df in [-1, 1] {
                    if let Some(t) = offset(from, df, dir) {
                        match self.sq[t as usize] {
                            Some(x) if x.color != c => pawn_to(t, MoveKind::Normal),
                            None if self.ep == Some(t) => pawn_to(t, MoveKind::EnPassant),
                            _ => {}
                        }
                    }
                }
            }
            Kind::Knight | Kind::King => {
                let steps = if o.kind == Kind::Knight { KNIGHT } else { KING };
                for (df, dr) in steps {
                    if let Some(t) = offset(from, df, dr)
                        && self.sq[t as usize].is_none_or(|x| x.color != c) {
                            push(t, MoveKind::Normal, None);
                        }
                }
                if o.kind == Kind::King {
                    for side in [KING_SIDE, QUEEN_SIDE] {
                        if let Some(m) = self.castle_move(from, side) {
                            out(m);
                        }
                    }
                }
            }
            Kind::Bishop | Kind::Rook | Kind::Queen => {
                let dirs: &[(i8, i8)] = match o.kind {
                    Kind::Bishop => &DIAG,
                    Kind::Rook => &ORTH,
                    _ => &[
                        (1, 1),
                        (1, -1),
                        (-1, 1),
                        (-1, -1),
                        (1, 0),
                        (-1, 0),
                        (0, 1),
                        (0, -1),
                    ],
                };
                for &(df, dr) in dirs {
                    let mut cur = from;
                    while let Some(t) = offset(cur, df, dr) {
                        match self.sq[t as usize] {
                            None => push(t, MoveKind::Normal, None),
                            Some(x) => {
                                if x.color != c {
                                    push(t, MoveKind::Normal, None);
                                }
                                break;
                            }
                        }
                        cur = t;
                    }
                }
            }
        }
    }

    /// Castling move for the king on `from`, if castling is allowed in this
    /// state (rights, empty squares, not castling out of/through check).
    pub fn castle_move(&self, from: Sq, side: usize) -> Option<CMove> {
        let c = self.turn;
        let home = c.home_rank();
        if from != sq(4, home) || !self.is(from, c, Kind::King) || !self.castle[c.idx()][side] {
            return None;
        }
        let (rook_file, empty, safe, to): (u8, &[u8], &[u8], u8) = if side == KING_SIDE {
            (7, &[5, 6], &[5, 6], 6)
        } else {
            (0, &[1, 2, 3], &[3, 2], 2)
        };
        if !self.is(sq(rook_file, home), c, Kind::Rook) {
            return None;
        }
        if empty.iter().any(|&f| self.sq[sq(f, home) as usize].is_some()) {
            return None;
        }
        if self.attacked(from, c.other()) || safe.iter().any(|&f| self.attacked(sq(f, home), c.other())) {
            return None;
        }
        Some(CMove {
            from,
            to: sq(to, home),
            promo: None,
            kind: MoveKind::Castle(side),
        })
    }

    pub fn apply(&self, m: CMove) -> Board {
        let mut b = self.clone();
        let o = b.sq[m.from as usize].take().expect("apply: empty from-square");
        if let Some(t) = b.sq[m.to as usize]
            && t.kind == Kind::King
        {
            b.kings[t.color.idx()] = 64;
        }
        match m.kind {
            MoveKind::EnPassant => {
                b.sq[sq(file_of(m.to), rank_of(m.from)) as usize] = None;
            }
            MoveKind::Castle(side) => {
                let home = rank_of(m.from);
                let (rf, rt) = if side == KING_SIDE { (7, 5) } else { (0, 3) };
                let r = b.sq[sq(rf, home) as usize].take();
                b.sq[sq(rt, home) as usize] = r;
            }
            _ => {}
        }
        if o.kind == Kind::King {
            b.kings[o.color.idx()] = m.to;
        }
        b.sq[m.to as usize] = Some(Occ {
            kind: m.promo.unwrap_or(o.kind),
            ..o
        });
        b.ep = (m.kind == MoveKind::DoublePush).then(|| (m.from + m.to) / 2);
        b.turn = self.turn.other();
        b
    }

    /// Fully legal: the mover's king is not left attacked.
    pub fn is_legal(&self, m: CMove) -> bool {
        !self.apply(m).in_check(self.turn)
    }

    pub fn has_legal_move(&self) -> bool {
        for s in 0..64u8 {
            if matches!(self.sq[s as usize], Some(o) if o.color == self.turn) {
                let mut found = false;
                self.each_pseudo(s, &mut |m| found = found || self.is_legal(m));
                if found {
                    return true;
                }
            }
        }
        false
    }

    /// Find the legal non-castling move `from -> to`. A missing promotion
    /// piece defaults to a queen.
    pub fn find_move(&self, from: Sq, to: Sq, promo: Option<Kind>) -> Option<CMove> {
        let mut found = None;
        self.each_pseudo(from, &mut |m| {
            if found.is_none()
                && m.to == to
                && !matches!(m.kind, MoveKind::Castle(_))
                && (m.promo == promo || (promo.is_none() && m.promo == Some(Kind::Queen)))
                && self.is_legal(m)
            {
                found = Some(m);
            }
        });
        found
    }

    pub fn find_castle(&self, from: Sq, side: usize) -> Option<CMove> {
        self.castle_move(from, side).filter(|&m| self.is_legal(m))
    }

    pub fn placement_fen(&self) -> String {
        let mut out = String::new();
        for r in (0..8).rev() {
            let mut empty = 0;
            for f in 0..8 {
                match self.sq[sq(f, r) as usize] {
                    None => empty += 1,
                    Some(o) => {
                        if empty > 0 {
                            out.push_str(&empty.to_string());
                            empty = 0;
                        }
                        out.push(o.kind.fen_char(o.color));
                    }
                }
            }
            if empty > 0 {
                out.push_str(&empty.to_string());
            }
            if r > 0 {
                out.push('/');
            }
        }
        out
    }
}
