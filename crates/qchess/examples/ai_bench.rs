//! Benchmark for the AI: `cargo run --release -p qchess --example ai_bench [secs]`
//! Uses the same allocator as the web server (mimalloc).
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
use qchess::{Game, ai};
use std::time::{Duration, Instant};
fn main() {
    let g = Game::from_history(&["e3/4", "e5", "Nf3", "Nc6", "Bb5/c4", "Ne7/f6", "0-0/", "Ra8b8"]).unwrap();
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if secs > 0 {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end { std::hint::black_box(ai::best_move(&g, ai::Config { depth: 3, time: None, ..Default::default() })); }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("game") {
        for margin in [0, 40] {
            let mut g = Game::new();
            let mut s = ai::Searcher::new(ai::Config { depth: 2, time: None, margin, ..Default::default() }).seed(1);
            let t = Instant::now();
            for _ in 0..60 {
                let Some(c) = s.choose(&g) else { break };
                g.play(&c.notation).unwrap();
            }
            println!("margin {margin}: 60 plies at depth 2 in {:.1?}, {} boards at the end", t.elapsed(), g.state_count());
        }
        return;
    }
    for (d, q, pruning) in [(2, true, false), (3, false, false), (3, true, false), (4, true, false), (4, true, true), (5, true, true)] {
        let t = Instant::now();
        let c = ai::best_move(&g, ai::Config { depth: d, time: None, quantum_moves: q, pruning, ..Default::default() }).unwrap();
        println!("depth {d} quantum={q} pruning={pruning}: {} nodes in {:.1?} -> {} ({})", c.nodes, t.elapsed(), c.notation, c.score);
    }
}
