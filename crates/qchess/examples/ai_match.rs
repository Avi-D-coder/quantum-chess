//! Play the AI with pruning against the AI without it, to check that the
//! speed doesn't cost strength:
//! `cargo run --release -p qchess --example ai_match -- [pairs] [depth] [plain depth]`.
//! The plain engine searches to `plain depth` (default: the same depth).
//!
//! Each pair of games starts from the same random opening (four random
//! legal plies), once with each engine as white. Games stop at checkmate,
//! stalemate, a repeated position or 160 plies; unfinished games are scored
//! by material.
use qchess::{Color, Game, Status, ai};
use std::collections::HashSet;
use std::time::{Duration, Instant};

fn material(g: &Game) -> i32 {
    // From white's side, halves of split pieces counted half.
    let v = |k: qchess::Kind| match k {
        qchess::Kind::Pawn => 100,
        qchess::Kind::Knight => 320,
        qchess::Kind::Bishop => 330,
        qchess::Kind::Rook => 500,
        qchess::Kind::Queen => 900,
        qchess::Kind::King => 0,
    };
    g.pieces()
        .iter()
        .map(|p| {
            let n = p.halves.len() as i32;
            let w: i32 = p.halves.iter().filter(|h| h.alive).map(|h| v(h.kind)).sum::<i32>() / n;
            if p.color == Color::White { w } else { -w }
        })
        .sum()
}

/// +1 white wins, -1 black wins, 0 draw; and the time each side used.
fn play(opening: &[String], white_prunes: bool, depth: i32, plain_depth: i32) -> (i32, [Duration; 2]) {
    let mut g = Game::from_history(opening).unwrap();
    let mk = |pruning| ai::Searcher::new(ai::Config { depth: if pruning { depth } else { plain_depth }, time: None, pruning, ..Default::default() });
    let mut engines = [mk(white_prunes), mk(!white_prunes)];
    let mut used = [Duration::ZERO; 2];
    let mut seen = HashSet::new();
    while g.status() == Status::Ongoing && g.history().len() < 160 {
        if !seen.insert(g.position_key()) {
            return (0, used);
        }
        let side = usize::from(g.to_move() == Color::Black);
        let t = Instant::now();
        let Some(c) = engines[side].choose(&g) else { break };
        used[side] += t.elapsed();
        g.play(&c.notation).unwrap();
    }
    let r = match g.status() {
        Status::Checkmate { winner: Color::White } => 1,
        Status::Checkmate { winner: Color::Black } => -1,
        Status::Ongoing => match material(&g) {
            m if m >= 300 => 1,
            m if m <= -300 => -1,
            _ => 0,
        },
        _ => 0,
    };
    (r, used)
}

fn main() {
    let pairs: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let depth: i32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let plain_depth: i32 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(depth);
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let (mut score, mut games) = (0.0f64, 0);
    let mut time = [Duration::ZERO; 2]; // [pruning, plain]
    for pair in 0..pairs {
        // A random opening.
        let mut g = Game::new();
        let mut opening = Vec::new();
        for _ in 0..4 {
            let kids = ai::children(&g, true);
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let c = &kids[(rng % kids.len() as u64) as usize];
            g.play(&c.notation).unwrap();
            opening.push(c.notation.clone());
        }
        for white_prunes in [true, false] {
            let (r, used) = play(&opening, white_prunes, depth, plain_depth);
            let (p, q) = if white_prunes { (0, 1) } else { (1, 0) };
            time[0] += used[p];
            time[1] += used[q];
            let pr = if white_prunes { r } else { -r };
            score += (pr as f64 + 1.0) / 2.0;
            games += 1;
            println!(
                "pair {pair} pruning as {}: {}",
                if white_prunes { "white" } else { "black" },
                ["pruning lost", "draw", "pruning won"][(pr + 1) as usize]
            );
        }
    }
    println!(
        "pruning scored {score}/{games}; time used: pruning {:.1?}, plain {:.1?} ({:.2}x)",
        time[0],
        time[1],
        time[1].as_secs_f64() / time[0].as_secs_f64()
    );
}
