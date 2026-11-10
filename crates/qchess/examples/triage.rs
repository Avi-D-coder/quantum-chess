//! Replays fuzz artifacts and groups their panic messages:
//! `cargo run --release --features fuzzing --example triage -- fuzz/artifacts/seq_token/*`
use std::collections::BTreeMap;
fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        let r = std::panic::catch_unwind(|| qchess::fuzzing::run(&data, 40));
        let msg = match r {
            Ok(()) => "ok".to_string(),
            Err(e) => e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default(),
        };
        let key: String = msg.lines().next().unwrap_or("").chars().take(70).collect();
        groups.entry(key).or_default().push(format!("{path}\n    {}", msg.replace('\n', "\n    ")));
    }
    for (k, v) in &groups {
        println!("== {} x{k}\n  {}", v.len(), v[0]);
    }
}
