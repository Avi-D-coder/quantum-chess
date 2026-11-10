//! Watch the AI play itself, with every move checked by the fuzz harness:
//! `cargo run --release -p qchess --features fuzzing --example selfplay -- [games] [depth] [plies]`
use std::time::Instant;
fn main() {
    let a: Vec<u64> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let (games, depth, plies) = (a.first().copied().unwrap_or(1), a.get(1).copied().unwrap_or(2) as i32, a.get(2).copied().unwrap_or(200) as usize);
    for seed in 1..=games {
        let t = Instant::now();
        let g = qchess::fuzzing::self_play(seed, plies, depth, 40);
        println!("game {seed}: {:?} after {} plies in {:.1?}", g.status(), g.history().len(), t.elapsed());
        println!("  {}", g.display_history().join(" "));
        println!("{}", g.board_text());
    }
}
