mod app;
mod auth;
mod bot;
mod db;
mod hub;
mod json;

use std::{sync::Arc, time::Duration};

/// The bot's search allocates a lot of small objects; mimalloc makes it
/// about a fifth faster than the system allocator.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() {
    let cfg = hub::Config::from_env();
    let db_path = cfg.get("DATABASE_URL").unwrap_or("qchess.db").trim_start_matches("sqlite:").to_string();
    let db = db::Db::open(&db_path).unwrap_or_else(|e| panic!("cannot open database {db_path}: {e}"));
    let state = Arc::new(hub::App::new(db, cfg));

    let ticker = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            interval.tick().await;
            ticker.tick();
        }
    });

    topcoat::start(app::router(state)).await.unwrap();
}
