//! Debugging helper: replays a game and shows what one more move does.
//! Run with: cargo run -p qchess --example probe -- <moves...> <next move>
//! e.g. `-- d4 e5 Nd2/c3 Nc6 Qd3`.

use qchess::Game;
fn main() {
    let h: Vec<String> = std::env::args().skip(1).collect();
    let (hist, mv) = h.split_at(h.len() - 1);
    let g = Game::from_history(hist).unwrap();
    println!("{}", g.board_text());
    for f in g.states_fen() { println!("  {f}"); }
    let from = &mv[0][mv[0].len().saturating_sub(4)..][..2];
    println!("targets({from}) = {:?}", g.targets(from));
    println!("play {} -> {:?}", mv[0], g.clone().play(&mv[0]).map(|r| r.collapses));
}
