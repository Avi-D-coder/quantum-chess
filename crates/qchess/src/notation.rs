//! Quantum algebraic notation.
//!
//! Ordinary SAN plus a `/` that splits a piece into two halves:
//!
//! | input         | meaning                                                      |
//! |---------------|--------------------------------------------------------------|
//! | `Nf3`, `exd5` | whole piece (both halves) moves, as in ordinary chess        |
//! | `Bb5/c4`      | halves go to b5 and c4                                        |
//! | `e3/4`        | shorthand: second square keeps the file (e3 and e4)          |
//! | `Nxd5/`       | one half moves to d5, the other half stays where it is       |
//! | `0-0/`        | quantum castling: rook moves, half the king castles          |
//! | `e8=Q/N`      | promote into two different quantum pieces on e8              |
//! | `Ncd5/eg6`    | disambiguation may be given for each half                    |
//!
//! Trailing `+`, `#`, `?` and `!` (e.g. `Qh5+?` for "Check?") are ignored.
//! When a move needs collapsing and there is more than one minimal way, the
//! mover may choose by appending `{...}` naming the half each collapsed piece
//! keeps: `{d6}`, `{Nc3}`, or `{xBd5}` for "the half captured on d5".

use crate::QError;
use crate::board::{KING_SIDE, QUEEN_SIDE};
use crate::types::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Hint {
    pub file: Option<u8>,
    pub rank: Option<u8>,
}

impl Hint {
    pub fn matches(&self, s: Sq) -> bool {
        self.file.is_none_or(|f| f == file_of(s)) && self.rank.is_none_or(|r| r == rank_of(s))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Second {
    /// `X/`: the other half stays put.
    Stay,
    /// `X/Y`: the other half goes to `to`.
    To {
        hint: Hint,
        to: Sq,
        promo: Option<Kind>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Intent {
    Castle {
        side: usize,
        split: bool,
    },
    Move {
        kind: Kind,
        hint: Hint,
        to: Sq,
        promo: Option<Kind>,
        second: Option<Second>,
    },
}

/// A collapse choice: keep the half of a piece that is on `sq`
/// (or that was captured on `sq` when `dead`). `num` picks the piece by its
/// number when several quantum pieces of that kind share the square, as in
/// `{N1d2}` (0 names an unnumbered piece).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollapseSpec {
    pub kind: Option<Kind>,
    pub num: Option<u8>,
    pub sq: Sq,
    pub dead: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub intent: Intent,
    pub collapse: Option<Vec<CollapseSpec>>,
}

/// Write a move back as notation that [`parse`] reads as the same intent:
/// pieces by letter (none for pawns), the disambiguation given, then the
/// squares, e.g. `Nb1c3`, `Bc1d2/e3`, `e7e8=Q/N`, `0-0/`.
pub fn format(intent: &Intent) -> String {
    fn hint(h: &Hint, out: &mut String) {
        if let Some(f) = h.file {
            out.push((b'a' + f) as char);
        }
        if let Some(r) = h.rank {
            out.push((b'1' + r) as char);
        }
    }
    fn promo(p: Option<Kind>, out: &mut String) {
        if let Some(k) = p {
            out.push('=');
            out.push(k.letter());
        }
    }
    let mut out = String::new();
    match *intent {
        Intent::Castle { side, split } => {
            out.push_str(if side == KING_SIDE { "0-0" } else { "0-0-0" });
            if split {
                out.push('/');
            }
        }
        Intent::Move { kind, hint: h, to, promo: p, second } => {
            if kind != Kind::Pawn {
                out.push(kind.letter());
            }
            hint(&h, &mut out);
            out.push_str(&sq_name(to));
            promo(p, &mut out);
            match second {
                None => {}
                Some(Second::Stay) => out.push('/'),
                Some(Second::To { hint: h2, to: t2, promo: p2 }) => {
                    out.push('/');
                    if h2 == Hint::default() && t2 == to && let Some(k) = p2 {
                        // Promotion-only second half: `e8=Q/N`.
                        out.push(k.letter());
                    } else {
                        hint(&h2, &mut out);
                        out.push_str(&sq_name(t2));
                        promo(p2, &mut out);
                    }
                }
            }
        }
    }
    out
}

fn err<T>(input: &str, why: &str) -> Result<T, QError> {
    Err(QError(format!("can't read move \"{input}\": {why}")))
}

pub fn parse(input: &str) -> Result<Parsed, QError> {
    let raw = input.trim();
    let (mv, collapse) = match raw.find('{') {
        Some(i) => {
            let rest = &raw[i + 1..];
            let Some(j) = rest.find('}') else {
                return err(raw, "missing '}'");
            };
            (raw[..i].trim(), Some(parse_collapse(raw, &rest[..j])?))
        }
        None => (raw, None),
    };
    let mv = mv.trim_end_matches(['+', '#', '?', '!']);
    if mv.is_empty() {
        return err(raw, "empty move");
    }
    let intent = parse_intent(raw, mv)?;
    Ok(Parsed { intent, collapse })
}

fn parse_collapse(raw: &str, body: &str) -> Result<Vec<CollapseSpec>, QError> {
    let mut out = Vec::new();
    for tok in body.split([',', ' ']).filter(|t| !t.is_empty()) {
        let mut t = tok;
        let dead = t.starts_with('x');
        if dead {
            t = &t[1..];
        }
        let kind = t.chars().next().and_then(Kind::from_letter);
        if kind.is_some() {
            t = &t[1..];
        }
        // Optional piece number: digits right before the square.
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        let num = if kind.is_some() && digits > 0 && t.len() == digits + 2 {
            let n = t[..digits].parse::<u8>().ok();
            t = &t[digits..];
            n
        } else {
            None
        };
        let Some(s) = parse_sq(t) else {
            return err(raw, &format!("bad collapse choice \"{tok}\""));
        };
        if kind.is_some() && digits > 0 && num.is_none() {
            return err(raw, &format!("bad collapse choice \"{tok}\""));
        }
        out.push(CollapseSpec { kind, num, sq: s, dead });
    }
    if out.is_empty() {
        return err(raw, "empty collapse choice {}");
    }
    Ok(out)
}

fn parse_intent(raw: &str, mv: &str) -> Result<Intent, QError> {
    let castle = mv.replace('O', "0");
    for (pat, side) in [("0-0-0", QUEEN_SIDE), ("0-0", KING_SIDE)] {
        if let Some(rest) = castle.strip_prefix(pat) {
            return match rest {
                "" => Ok(Intent::Castle { side, split: false }),
                "/" => Ok(Intent::Castle { side, split: true }),
                _ => err(raw, "castling can only be followed by '/'"),
            };
        }
    }

    let (main, second) = match mv.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (mv, None),
    };

    let mut chars: Vec<char> = main.chars().filter(|&c| c != 'x' && c != '-' && c != ':').collect();
    let kind = match chars.first().and_then(|&c| Kind::from_letter(c)) {
        Some(k) if k != Kind::Pawn => {
            chars.remove(0);
            k
        }
        _ => Kind::Pawn,
    };
    let promo = take_promo(&mut chars);
    let (hint, to) = match split_square(&chars) {
        Some(v) => v,
        None => return err(raw, "expected a destination square like e4"),
    };
    if promo.is_some() && kind != Kind::Pawn {
        return err(raw, "only pawns promote");
    }

    let second = match second {
        None => None,
        Some(s) => Some(parse_second(raw, s, to)?),
    };
    Ok(Intent::Move {
        kind,
        hint,
        to,
        promo,
        second,
    })
}

/// Strip a trailing promotion (`=Q` or bare `Q`).
fn take_promo(chars: &mut Vec<char>) -> Option<Kind> {
    let last = *chars.last()?;
    let k = Kind::from_letter(last).filter(|k| !matches!(k, Kind::Pawn | Kind::King))?;
    chars.pop();
    if chars.last() == Some(&'=') {
        chars.pop();
    }
    Some(k)
}

/// `[file][rank]square` → (hint, square).
fn split_square(chars: &[char]) -> Option<(Hint, Sq)> {
    if chars.len() < 2 || chars.len() > 4 {
        return None;
    }
    let n = chars.len();
    let s: String = chars[n - 2..].iter().collect();
    let to = parse_sq(&s)?;
    let mut hint = Hint::default();
    for &c in &chars[..n - 2] {
        match c {
            'a'..='h' if hint.file.is_none() => hint.file = Some(c as u8 - b'a'),
            '1'..='8' if hint.rank.is_none() => hint.rank = Some(c as u8 - b'1'),
            _ => return None,
        }
    }
    Some((hint, to))
}

fn parse_second(raw: &str, s: &str, first: Sq) -> Result<Second, QError> {
    let mut chars: Vec<char> = s.chars().filter(|&c| c != 'x' && c != '-' && c != ':').collect();
    if chars.is_empty() {
        // `e4/` stays; `e4/x` or `e4/-` names a second move without a square.
        return if s.trim().is_empty() { Ok(Second::Stay) } else { err(raw, "expected a square after '/'") };
    }
    // Promotion-only: `e8=Q/N` or `e8=Q/=N`.
    let promo_only: String = chars.iter().filter(|&&c| c != '=').collect();
    if promo_only.len() == 1
        && let Some(k) = promo_only.chars().next().and_then(Kind::from_letter)
            && matches!(k, Kind::Queen | Kind::Rook | Kind::Bishop | Kind::Knight) {
                return Ok(Second::To {
                    hint: Hint::default(),
                    to: first,
                    promo: Some(k),
                });
            }
    // Optional repeated piece letter: `Bb5/Bc4`.
    if chars.first().is_some_and(|c| c.is_ascii_uppercase()) {
        chars.remove(0);
    }
    let promo = take_promo(&mut chars);
    let to = match chars.as_slice() {
        [f @ 'a'..='h'] => sq(*f as u8 - b'a', rank_of(first)),
        [r @ '1'..='8'] => sq(file_of(first), *r as u8 - b'1'),
        [] if promo.is_some() => first,
        _ => match split_square(&chars) {
            Some((hint, to)) => return Ok(Second::To { hint, to, promo }),
            None => return err(raw, "can't read the part after '/'"),
        },
    };
    Ok(Second::To {
        hint: Hint::default(),
        to,
        promo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(s: &str) -> Intent {
        parse(s).unwrap().intent
    }

    #[test]
    fn sheet_examples() {
        let e3 = parse_sq("e3").unwrap();
        let e4 = parse_sq("e4").unwrap();
        assert_eq!(
            mv("e3/4"),
            Intent::Move {
                kind: Kind::Pawn,
                hint: Hint::default(),
                to: e3,
                promo: None,
                second: Some(Second::To {
                    hint: Hint::default(),
                    to: e4,
                    promo: None
                })
            }
        );
        assert!(matches!(mv("Bb5/c4"), Intent::Move { kind: Kind::Bishop, second: Some(Second::To { .. }), .. }));
        assert_eq!(mv("0-0/"), Intent::Castle { side: KING_SIDE, split: true });
        assert_eq!(mv("O-O-O"), Intent::Castle { side: QUEEN_SIDE, split: false });
        assert!(matches!(mv("Nxd5/"), Intent::Move { second: Some(Second::Stay), .. }));
        assert!(matches!(
            mv("e8=Q/N"),
            Intent::Move { promo: Some(Kind::Queen), second: Some(Second::To { promo: Some(Kind::Knight), .. }), .. }
        ));
        assert!(matches!(mv("exd5"), Intent::Move { hint: Hint { file: Some(4), rank: None }, .. }));
        assert!(matches!(mv("Qh5+?"), Intent::Move { kind: Kind::Queen, .. }));
        assert!(matches!(mv("e4/"), Intent::Move { second: Some(Second::Stay), .. }));
        for bad in ["e4/x", "e4/-", "Nf3/::"] {
            assert!(parse(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn collapse_choice() {
        let p = parse("Nxd5/ {xPd5, Nc3}").unwrap();
        let c = p.collapse.unwrap();
        assert_eq!(c.len(), 2);
        assert!(c[0].dead && c[0].kind == Some(Kind::Pawn));
        assert!(!c[1].dead);
        let c = parse("Nb1a3/d2 {N1d2,N2a3}").unwrap().collapse.unwrap();
        assert_eq!((c[0].num, c[1].num), (Some(1), Some(2)));
        assert!(parse("Nd5 {N999d2}").is_err());
    }

    #[test]
    fn format_reads_back_as_the_same_move() {
        for m in ["Nb1c3", "Bc1d2/e3", "e7e8=Q/N", "e7e8=Q/", "e7e8=N", "0-0/", "0-0-0", "e2e4/", "Nb1a3/d2c4", "e2e3/e4e5", "b7c8=Q/b8=Q", "Kd4"] {
            let i = mv(m);
            assert_eq!(mv(&format(&i)), i, "{m} -> {}", format(&i));
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("Zz9").is_err());
        assert!(parse("0-0x").is_err());
        assert!(parse("").is_err());
    }
}
