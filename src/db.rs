//! SQLite storage: matches, players, follows.

use rusqlite::{params, Connection};

use crate::model::{Filters, MatchSummary, PlayerSummary};

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS matches (
            uuid TEXT PRIMARY KEY,
            game_version TEXT NOT NULL DEFAULT '',
            map TEXT NOT NULL DEFAULT '',
            game_type TEXT NOT NULL DEFAULT '',
            duration_frames INTEGER NOT NULL DEFAULT 0,
            matchup TEXT NOT NULL DEFAULT '',
            matchup_norm TEXT NOT NULL DEFAULT '',
            uploaded_by TEXT NOT NULL DEFAULT '',
            uploaded_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS players (
            match_uuid TEXT NOT NULL REFERENCES matches(uuid),
            name TEXT NOT NULL,
            race TEXT NOT NULL DEFAULT '',
            slot INTEGER NOT NULL,
            result TEXT NOT NULL DEFAULT 'unknown',
            PRIMARY KEY (match_uuid, slot)
        );
        CREATE TABLE IF NOT EXISTS follows (
            account TEXT NOT NULL,
            player TEXT NOT NULL,
            PRIMARY KEY (account, player)
        );
        CREATE INDEX IF NOT EXISTS idx_players_name ON players(name);
        CREATE INDEX IF NOT EXISTS idx_players_race ON players(race);
        CREATE INDEX IF NOT EXISTS idx_matches_uploaded_at ON matches(uploaded_at);
        CREATE INDEX IF NOT EXISTS idx_matches_matchup_norm ON matches(matchup_norm);
        "#,
    )
}

pub struct InsertMatch {
    pub uuid: String,
    pub game_version: String,
    pub map: String,
    pub game_type: String,
    pub duration_frames: i64,
    pub matchup: String,
    pub matchup_norm: String,
    pub uploaded_by: String,
    pub players: Vec<(String, String, i64, String)>, // name, race, slot, result
}

pub fn insert_match(conn: &mut Connection, m: &InsertMatch) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO matches (uuid, game_version, map, game_type, duration_frames, matchup, matchup_norm, uploaded_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            m.uuid,
            m.game_version,
            m.map,
            m.game_type,
            m.duration_frames,
            m.matchup,
            m.matchup_norm,
            m.uploaded_by
        ],
    )?;
    for (name, race, slot, result) in &m.players {
        tx.execute(
            "INSERT INTO players (match_uuid, name, race, slot, result) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![m.uuid, name, race, slot, result],
        )?;
    }
    tx.commit()
}

/// List matches with the given filters. Returns (rows, total-without-limit).
pub fn list_matches(conn: &Connection, f: &Filters) -> rusqlite::Result<(Vec<MatchSummary>, i64)> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(p) = &f.player {
        where_parts.push(format!(
            "m.uuid IN (SELECT match_uuid FROM players WHERE name LIKE ?{} COLLATE NOCASE)",
            args.len() + 1
        ));
        args.push(Box::new(format!("%{}%", p)));
    }
    if let Some(r) = &f.race {
        where_parts.push(format!(
            "m.uuid IN (SELECT match_uuid FROM players WHERE race = ?{} COLLATE NOCASE)",
            args.len() + 1
        ));
        args.push(Box::new(r.clone()));
    }
    if let Some(r) = &f.result {
        where_parts.push(format!(
            "m.uuid IN (SELECT match_uuid FROM players WHERE result = ?{} COLLATE NOCASE)",
            args.len() + 1
        ));
        args.push(Box::new(r.clone()));
    }
    if let Some(m) = &f.matchup {
        where_parts.push(format!("m.matchup_norm = ?{}", args.len() + 1));
        args.push(Box::new(crate::model::normalize_matchup(m)));
    }
    if let Some(d) = &f.from {
        where_parts.push(format!("m.uploaded_at >= ?{}", args.len() + 1));
        args.push(Box::new(d.clone()));
    }
    if let Some(d) = &f.to {
        where_parts.push(format!("m.uploaded_at <= ?{}", args.len() + 1));
        args.push(Box::new(d.clone()));
    }
    if let Some(acc) = &f.followed_by {
        where_parts.push(format!(
            "m.uuid IN (
                SELECT players.match_uuid FROM players
                JOIN follows ON follows.player = players.name COLLATE NOCASE
                WHERE follows.account = ?{} COLLATE NOCASE
            )",
            args.len() + 1
        ));
        args.push(Box::new(acc.clone()));
    }

    let where_sql = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let total: i64 = {
        let sql = format!("SELECT COUNT(*) FROM matches m {where_sql}");
        conn.query_row(&sql, rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), |r| r.get(0))?
    };

    let limit = f.limit.clamp(1, 200);
    let offset = f.offset.max(0);
    let sql = format!(
        "SELECT m.uuid, m.game_version, m.map, m.game_type, m.duration_frames, m.matchup,
                m.uploaded_by, m.uploaded_at
         FROM matches m {where_sql}
         ORDER BY m.uploaded_at DESC, m.uuid
         LIMIT {limit} OFFSET {offset}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    })?;

    let mut matches = Vec::new();
    for row in rows {
        let (uuid, game_version, map, game_type, duration_frames, matchup, uploaded_by, uploaded_at) =
            row?;
        let players = players_for(conn, &uuid)?;
        matches.push(MatchSummary {
            uuid,
            game_version,
            map,
            game_type,
            duration_frames,
            matchup,
            uploaded_by,
            uploaded_at,
            players,
        });
    }
    Ok((matches, total))
}

fn players_for(conn: &Connection, uuid: &str) -> rusqlite::Result<Vec<PlayerSummary>> {
    let mut stmt = conn.prepare(
        "SELECT name, race, slot, result FROM players WHERE match_uuid = ?1 ORDER BY slot",
    )?;
    let rows = stmt.query_map([uuid], |r| {
        Ok(PlayerSummary {
            name: r.get(0)?,
            race: r.get(1)?,
            slot: r.get(2)?,
            result: r.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn match_exists(conn: &Connection, uuid: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM matches WHERE uuid = ?1",
        [uuid],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn follow(conn: &Connection, account: &str, player: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO follows (account, player) VALUES (?1, ?2)",
        params![account, player],
    )?;
    Ok(())
}

pub fn unfollow(conn: &Connection, account: &str, player: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM follows WHERE account = ?1 AND player = ?2 COLLATE NOCASE",
        params![account, player],
    )?;
    Ok(())
}

pub fn following(conn: &Connection, account: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT player FROM follows WHERE account = ?1 ORDER BY player")?;
    let rows = stmt.query_map([account], |r| r.get(0))?;
    rows.collect()
}
