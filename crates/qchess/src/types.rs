//! Basic chess vocabulary: colors, piece kinds and squares.

/// Side to move / owner of a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn other(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    pub(crate) fn idx(self) -> usize {
        match self {
            Color::White => 0,
            Color::Black => 1,
        }
    }

    pub(crate) fn home_rank(self) -> u8 {
        match self {
            Color::White => 0,
            Color::Black => 7,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Color::White => "white",
            Color::Black => "black",
        }
    }
}

/// Kind of a (half of a) piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl Kind {
    /// SAN letter (uppercase). Pawns use `P` where a letter is needed.
    pub fn letter(self) -> char {
        match self {
            Kind::Pawn => 'P',
            Kind::Knight => 'N',
            Kind::Bishop => 'B',
            Kind::Rook => 'R',
            Kind::Queen => 'Q',
            Kind::King => 'K',
        }
    }

    pub fn from_letter(c: char) -> Option<Kind> {
        Some(match c {
            'P' => Kind::Pawn,
            'N' => Kind::Knight,
            'B' => Kind::Bishop,
            'R' => Kind::Rook,
            'Q' => Kind::Queen,
            'K' => Kind::King,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Pawn => "pawn",
            Kind::Knight => "knight",
            Kind::Bishop => "bishop",
            Kind::Rook => "rook",
            Kind::Queen => "queen",
            Kind::King => "king",
        }
    }

    pub(crate) fn fen_char(self, color: Color) -> char {
        let c = self.letter();
        match color {
            Color::White => c,
            Color::Black => c.to_ascii_lowercase(),
        }
    }
}

/// Square index 0..64, `a1 = 0`, `b1 = 1`, ..., `h8 = 63`.
pub type Sq = u8;

pub fn sq(file: u8, rank: u8) -> Sq {
    rank * 8 + file
}

pub fn file_of(s: Sq) -> u8 {
    s % 8
}

pub fn rank_of(s: Sq) -> u8 {
    s / 8
}

pub fn sq_name(s: Sq) -> String {
    format!("{}{}", (b'a' + file_of(s)) as char, rank_of(s) + 1)
}

pub fn parse_sq(t: &str) -> Option<Sq> {
    let b = t.as_bytes();
    if b.len() != 2 {
        return None;
    }
    let f = b[0].wrapping_sub(b'a');
    let r = b[1].wrapping_sub(b'1');
    (f < 8 && r < 8).then(|| sq(f, r))
}

/// Step from a square by a file/rank delta, staying on the board.
pub(crate) fn offset(s: Sq, df: i8, dr: i8) -> Option<Sq> {
    let f = file_of(s) as i8 + df;
    let r = rank_of(s) as i8 + dr;
    ((0..8).contains(&f) && (0..8).contains(&r)).then(|| sq(f as u8, r as u8))
}
