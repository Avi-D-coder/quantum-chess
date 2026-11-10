//! SQLite storage: users, OAuth identities, sessions, ratings, games, moves.

use std::sync::Mutex;

use glicko2::Rating;
use rusqlite::{Connection, OptionalExtension, params};

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: i64,
    pub username: Option<String>,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

impl User {
    /// The name shown in games: username, falling back to the display name.
    pub fn name(&self) -> &str {
        self.username.as_deref().unwrap_or(&self.display_name)
    }
}

#[derive(Clone, Debug)]
pub struct GameRow {
    pub id: String,
    pub white_id: i64,
    pub black_id: i64,
    pub white_name: String,
    pub black_name: String,
    pub initial_ms: u64,
    pub increment_ms: u64,
    pub status: String,
    pub result: Option<String>,
    pub termination: Option<String>,
    #[allow(dead_code)]
    pub created_at: i64,
    pub rated: bool,
}

#[derive(Clone, Debug)]
pub struct MoveRow {
    pub ply: usize,
    pub notation: String,
    pub collapses: Vec<String>,
    pub white_ms: u64,
    pub black_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
}

#[derive(Debug)]
pub enum UsernameError {
    Invalid,
    Taken,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  username TEXT UNIQUE COLLATE NOCASE,
  display_name TEXT NOT NULL,
  avatar_url TEXT,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS identities(
  provider TEXT NOT NULL,
  subject TEXT NOT NULL,
  user_id INTEGER NOT NULL REFERENCES users(id),
  email TEXT,
  PRIMARY KEY(provider, subject)
);
CREATE TABLE IF NOT EXISTS passwords(
  user_id INTEGER PRIMARY KEY REFERENCES users(id),
  hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions(
  token_hash BLOB PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ratings(
  user_id INTEGER NOT NULL REFERENCES users(id),
  speed TEXT NOT NULL,
  rating REAL NOT NULL,
  rd REAL NOT NULL,
  volatility REAL NOT NULL,
  games INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(user_id, speed)
);
CREATE TABLE IF NOT EXISTS games(
  id TEXT PRIMARY KEY,
  white_id INTEGER NOT NULL REFERENCES users(id),
  black_id INTEGER NOT NULL REFERENCES users(id),
  initial_ms INTEGER NOT NULL,
  increment_ms INTEGER NOT NULL,
  status TEXT NOT NULL,
  result TEXT,
  termination TEXT,
  created_at INTEGER NOT NULL,
  ended_at INTEGER,
  rated INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS games_white ON games(white_id, created_at);
CREATE INDEX IF NOT EXISTS games_black ON games(black_id, created_at);
CREATE TABLE IF NOT EXISTS moves(
  game_id TEXT NOT NULL REFERENCES games(id),
  ply INTEGER NOT NULL,
  notation TEXT NOT NULL,
  collapses_json TEXT NOT NULL,
  white_ms INTEGER NOT NULL,
  black_ms INTEGER NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY(game_id, ply)
);
CREATE TABLE IF NOT EXISTS bots(
  level TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id)
);
";

fn user_from(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User { id: r.get(0)?, username: r.get(1)?, display_name: r.get(2)?, avatar_url: r.get(3)? })
}

const GAME_COLS: &str = "g.id, g.white_id, g.black_id, COALESCE(w.username, w.display_name), \
     COALESCE(b.username, b.display_name), g.initial_ms, g.increment_ms, g.status, g.result, \
     g.termination, g.created_at, g.rated";
const GAME_JOIN: &str = "FROM games g JOIN users w ON w.id = g.white_id JOIN users b ON b.id = g.black_id";

fn game_from(r: &rusqlite::Row) -> rusqlite::Result<GameRow> {
    Ok(GameRow {
        id: r.get(0)?,
        white_id: r.get(1)?,
        black_id: r.get(2)?,
        white_name: r.get(3)?,
        black_name: r.get(4)?,
        initial_ms: r.get::<_, i64>(5)? as u64,
        increment_ms: r.get::<_, i64>(6)? as u64,
        status: r.get(7)?,
        result: r.get(8)?,
        termination: r.get(9)?,
        created_at: r.get(10)?,
        rated: r.get::<_, i64>(11)? != 0,
    })
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Db> {
        let conn = if path == ":memory:" { Connection::open_in_memory()? } else { Connection::open(path)? };
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA)?;
        // Live games do not survive a restart: close any left open.
        conn.execute(
            "UPDATE games SET status='ended', result='1/2-1/2', termination='abort', ended_at=?1 WHERE status='ongoing'",
            [now_secs()],
        )?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- users & identities

    /// Finds or creates the user for an OAuth identity.
    pub fn upsert_identity(
        &self,
        provider: &str,
        subject: &str,
        display_name: &str,
        avatar: Option<&str>,
        email: Option<&str>,
    ) -> rusqlite::Result<User> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT user_id FROM identities WHERE provider=?1 AND subject=?2",
                params![provider, subject],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO users(display_name, avatar_url, created_at) VALUES (?1, ?2, ?3)",
                    params![display_name, avatar, now_secs()],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO identities(provider, subject, user_id, email) VALUES (?1,?2,?3,?4)",
                    params![provider, subject, id, email],
                )?;
                id
            }
        };
        let user = tx.query_row(
            "SELECT id, username, display_name, avatar_url FROM users WHERE id=?1",
            [id],
            user_from,
        )?;
        tx.commit()?;
        Ok(user)
    }

    #[cfg_attr(not(feature = "test-auth"), allow(dead_code))]
    pub fn user(&self, id: i64) -> Option<User> {
        self.c()
            .query_row("SELECT id, username, display_name, avatar_url FROM users WHERE id=?1", [id], user_from)
            .optional()
            .ok()
            .flatten()
    }

    pub fn user_by_name(&self, name: &str) -> Option<User> {
        self.c()
            .query_row(
                "SELECT id, username, display_name, avatar_url FROM users WHERE username=?1",
                [name],
                user_from,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The computer player for `level`, created on first use. It gets
    /// `name` as its username, or `name` plus a number if that is taken.
    pub fn bot_user(&self, level: &str, name: &str) -> rusqlite::Result<User> {
        let existing: Option<i64> =
            self.c().query_row("SELECT user_id FROM bots WHERE level=?1", [level], |r| r.get(0)).optional()?;
        if let Some(u) = existing.and_then(|id| self.user(id)) {
            return Ok(u);
        }
        let c = self.c();
        let mut n = 0;
        let id = loop {
            let candidate = if n == 0 { name.to_string() } else { format!("{name}{n}") };
            match c.execute(
                "INSERT INTO users(username, display_name, created_at) VALUES (?1, ?1, ?2)",
                params![candidate, now_secs()],
            ) {
                Ok(_) => break c.last_insert_rowid(),
                Err(_) if n < 50 => n += 1,
                Err(e) => return Err(e),
            }
        };
        c.execute("INSERT OR REPLACE INTO bots(level, user_id) VALUES (?1, ?2)", params![level, id])?;
        drop(c);
        self.user(id).ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn valid_username(name: &str) -> bool {
        (2..=20).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    pub fn set_username(&self, id: i64, name: &str) -> Result<(), UsernameError> {
        if !Self::valid_username(name) {
            return Err(UsernameError::Invalid);
        }
        match self.c().execute("UPDATE users SET username=?1 WHERE id=?2 AND username IS NULL", params![name, id]) {
            Ok(_) => Ok(()),
            Err(_) => Err(UsernameError::Taken),
        }
    }

    // ---- test-only password accounts

    /// Creates a user with a username and password hash.
    #[cfg(feature = "test-auth")]
    pub fn create_password_user(&self, username: &str, hash: &str) -> Result<User, UsernameError> {
        if !Self::valid_username(username) {
            return Err(UsernameError::Invalid);
        }
        let mut c = self.c();
        let tx = c.transaction().map_err(|_| UsernameError::Taken)?;
        tx.execute(
            "INSERT INTO users(username, display_name, created_at) VALUES (?1, ?1, ?2)",
            params![username, now_secs()],
        )
        .map_err(|_| UsernameError::Taken)?;
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO passwords(user_id, hash) VALUES (?1, ?2)", params![id, hash])
            .map_err(|_| UsernameError::Taken)?;
        tx.commit().map_err(|_| UsernameError::Taken)?;
        drop(c);
        self.user(id).ok_or(UsernameError::Taken)
    }

    /// The user and password hash for a username, if it has a password.
    #[cfg(feature = "test-auth")]
    pub fn password_login(&self, username: &str) -> Option<(User, String)> {
        let user = self.user_by_name(username)?;
        let hash: String = self
            .c()
            .query_row("SELECT hash FROM passwords WHERE user_id=?1", [user.id], |r| r.get(0))
            .optional()
            .ok()
            .flatten()?;
        Some((user, hash))
    }

    // ---- sessions

    pub fn create_session(&self, hash: &[u8], user_id: i64, expires_at: i64) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO sessions(token_hash, user_id, expires_at) VALUES (?1,?2,?3)",
            params![hash, user_id, expires_at],
        )?;
        Ok(())
    }

    pub fn session_user(&self, hash: &[u8]) -> Option<User> {
        self.c()
            .query_row(
                "SELECT u.id, u.username, u.display_name, u.avatar_url FROM sessions s \
                 JOIN users u ON u.id = s.user_id WHERE s.token_hash=?1 AND s.expires_at > ?2",
                params![hash, now_secs()],
                user_from,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn delete_session(&self, hash: &[u8]) {
        let _ = self.c().execute("DELETE FROM sessions WHERE token_hash=?1", [hash]);
    }

    // ---- ratings

    pub fn rating(&self, user_id: i64, speed: &str) -> (Rating, u32) {
        self.c()
            .query_row(
                "SELECT rating, rd, volatility, games FROM ratings WHERE user_id=?1 AND speed=?2",
                params![user_id, speed],
                |r| Ok((Rating::new(r.get(0)?, r.get(1)?, r.get(2)?), r.get::<_, u32>(3)?)),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or((Rating::default(), 0))
    }

    pub fn set_rating(&self, user_id: i64, speed: &str, r: Rating, games: u32) {
        let _ = self.c().execute(
            "INSERT INTO ratings(user_id, speed, rating, rd, volatility, games) VALUES (?1,?2,?3,?4,?5,?6) \
             ON CONFLICT(user_id, speed) DO UPDATE SET rating=?3, rd=?4, volatility=?5, games=?6",
            params![user_id, speed, r.rating, r.deviation, r.volatility, games],
        );
    }

    pub fn ratings(&self, user_id: i64) -> Vec<(String, Rating, u32)> {
        let c = self.c();
        let Ok(mut st) =
            c.prepare("SELECT speed, rating, rd, volatility, games FROM ratings WHERE user_id=?1 ORDER BY speed")
        else {
            return vec![];
        };
        st.query_map([user_id], |r| {
            Ok((r.get::<_, String>(0)?, Rating::new(r.get(1)?, r.get(2)?, r.get(3)?), r.get::<_, u32>(4)?))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    // ---- games

    pub fn insert_game(
        &self,
        id: &str,
        white: i64,
        black: i64,
        initial_ms: u64,
        increment_ms: u64,
        rated: bool,
    ) -> rusqlite::Result<()> {
        self.c().execute(
            "INSERT INTO games(id, white_id, black_id, initial_ms, increment_ms, status, created_at, rated) \
             VALUES (?1,?2,?3,?4,?5,'ongoing',?6,?7)",
            params![id, white, black, initial_ms as i64, increment_ms as i64, now_secs(), rated as i64],
        )?;
        Ok(())
    }

    pub fn insert_move(&self, game: &str, m: &MoveRow, at_ms: u64) {
        let json = serde_json::to_string(&m.collapses).unwrap_or_else(|_| "[]".into());
        let _ = self.c().execute(
            "INSERT OR REPLACE INTO moves(game_id, ply, notation, collapses_json, white_ms, black_ms, at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![game, m.ply as i64, m.notation, json, m.white_ms as i64, m.black_ms as i64, at_ms as i64],
        );
    }

    pub fn finish_game(&self, id: &str, result: &str, termination: &str) {
        let _ = self.c().execute(
            "UPDATE games SET status='ended', result=?2, termination=?3, ended_at=?4 WHERE id=?1",
            params![id, result, termination, now_secs()],
        );
    }

    pub fn game(&self, id: &str) -> Option<GameRow> {
        let sql = format!("SELECT {GAME_COLS} {GAME_JOIN} WHERE g.id=?1");
        self.c().query_row(&sql, [id], game_from).optional().ok().flatten()
    }

    pub fn moves(&self, id: &str) -> Vec<MoveRow> {
        let c = self.c();
        let Ok(mut st) = c.prepare(
            "SELECT ply, notation, collapses_json, white_ms, black_ms FROM moves WHERE game_id=?1 ORDER BY ply",
        ) else {
            return vec![];
        };
        st.query_map([id], |r| {
            let json: String = r.get(2)?;
            Ok(MoveRow {
                ply: r.get::<_, i64>(0)? as usize,
                notation: r.get(1)?,
                collapses: serde_json::from_str(&json).unwrap_or_default(),
                white_ms: r.get::<_, i64>(3)? as u64,
                black_ms: r.get::<_, i64>(4)? as u64,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// Finished games of a user, newest first.
    pub fn user_games(&self, user_id: i64, limit: usize, offset: usize) -> Vec<GameRow> {
        let sql = format!(
            "SELECT {GAME_COLS} {GAME_JOIN} WHERE (g.white_id=?1 OR g.black_id=?1) AND g.status='ended' \
             ORDER BY g.created_at DESC, g.rowid DESC LIMIT ?2 OFFSET ?3"
        );
        let c = self.c();
        let Ok(mut st) = c.prepare(&sql) else { return vec![] };
        st.query_map(params![user_id, limit as i64, offset as i64], game_from)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn stats(&self, user_id: i64) -> Stats {
        let mut s = Stats::default();
        let c = self.c();
        let Ok(mut st) = c.prepare(
            "SELECT white_id, result FROM games WHERE (white_id=?1 OR black_id=?1) AND status='ended' \
             AND termination != 'abort'",
        ) else {
            return s;
        };
        if let Ok(rows) = st.query_map([user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))) {
            for (white, result) in rows.flatten() {
                match (result.as_deref(), white == user_id) {
                    (Some("1-0"), true) | (Some("0-1"), false) => s.wins += 1,
                    (Some("1-0"), false) | (Some("0-1"), true) => s.losses += 1,
                    (Some(_), _) => s.draws += 1,
                    _ => {}
                }
            }
        }
        s
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_session_and_game_roundtrip() {
        let db = Db::open(":memory:").unwrap();
        let a = db.upsert_identity("github", "1", "Ann", None, None).unwrap();
        let again = db.upsert_identity("github", "1", "Ann2", None, None).unwrap();
        assert_eq!(a.id, again.id);
        let b = db.upsert_identity("lichess", "bob", "Bob", None, None).unwrap();
        db.set_username(a.id, "ann").unwrap();
        assert!(matches!(db.set_username(b.id, "ANN"), Err(UsernameError::Taken)));
        assert!(matches!(db.set_username(b.id, "x"), Err(UsernameError::Invalid)));

        db.create_session(&[1, 2, 3], a.id, now_secs() + 100).unwrap();
        assert_eq!(db.session_user(&[1, 2, 3]).unwrap().id, a.id);
        db.create_session(&[9], a.id, now_secs() - 1).unwrap();
        assert!(db.session_user(&[9]).is_none());
        db.delete_session(&[1, 2, 3]);
        assert!(db.session_user(&[1, 2, 3]).is_none());

        db.insert_game("g1", a.id, b.id, 180_000, 2000, true).unwrap();
        db.insert_move(
            "g1",
            &MoveRow { ply: 1, notation: "e2e4".into(), collapses: vec![], white_ms: 1, black_ms: 2 },
            5,
        );
        assert_eq!(db.moves("g1").len(), 1);
        assert_eq!(db.game("g1").unwrap().status, "ongoing");
        db.finish_game("g1", "1-0", "resign");
        assert_eq!(db.user_games(a.id, 10, 0).len(), 1);
        let s = db.stats(a.id);
        assert_eq!((s.wins, s.losses, s.draws), (1, 0, 0));
        assert_eq!(db.stats(b.id).losses, 1);

        let (r, n) = db.rating(a.id, "blitz");
        assert_eq!(n, 0);
        db.set_rating(a.id, "blitz", Rating::new(1600.0, 100.0, 0.06), 3);
        let (r2, n2) = db.rating(a.id, "blitz");
        assert_eq!((r2.rating, n2), (1600.0, 3));
        let _ = r;
        assert_eq!(db.ratings(a.id).len(), 1);
    }

    #[cfg(feature = "test-auth")]
    #[test]
    fn password_users() {
        let db = Db::open(":memory:").unwrap();
        let u = db.create_password_user("pat", "hash1").unwrap();
        assert_eq!(u.username.as_deref(), Some("pat"));
        assert!(matches!(db.create_password_user("PAT", "h"), Err(UsernameError::Taken)));
        assert!(matches!(db.create_password_user("x", "h"), Err(UsernameError::Invalid)));
        let (found, hash) = db.password_login("pat").unwrap();
        assert_eq!((found.id, hash.as_str()), (u.id, "hash1"));
        assert!(db.password_login("nobody").is_none());
    }
}
