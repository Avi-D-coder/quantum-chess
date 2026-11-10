//! Plays the opening from the rule sheet and prints each position.
//! Run with: cargo run -p qchess --example sheet

fn main() {
    let mut g = qchess::Game::new();
    for m in ["e3/4", "e5", "Nf3", "Nc6", "Bb5/c4", "Ne7/f6", "0-0/"] {
        let r = g.play(m).expect("legal move");
        println!("{m}  collapses: {:?}  check: {:?}", r.collapses, r.check);
    }
    println!("\n{}", g.board_text());
    println!("{} possible board states", g.state_count());
}
