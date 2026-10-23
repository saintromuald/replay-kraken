//! SQLite storage: matches, players, follows, discord identities, stats.

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{Filters, MatchSummary, PlayerStats, PlayerSummary};

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
            discord_id TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (match_uuid, slot)
        );
        CREATE TABLE IF NOT EXISTS follows (
            account TEXT NOT NULL,
            player TEXT NOT NULL,
            PRIMARY KEY (account, player)
        );
        -- Account identity: upload account -> Discord id (OAuth later; config now).
        CREATE TABLE IF NOT EXISTS accounts (
            account TEXT PRIMARY KEY,
            discord_id TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_players_name ON players(name);
        CREATE INDEX IF NOT EXISTS idx_players_race ON players(race);
        CREATE INDEX IF NOT EXISTS idx_matches_uploaded_at ON matches(uploaded_at);
        CREATE INDEX IF NOT EXISTS idx_matches_matchup_norm ON matches(matchup_norm);
        "#,
    )?;
    // Migration for databases created before the discord_id column existed. Must
    // run before the discord index is created (it references the column).
    let has_col: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('players') WHERE name = 'discord_id'",
        [],
        |r| r.get(0),
    )?;
    if has_col == 0 {
        conn.execute(
            "ALTER TABLE players ADD COLUMN discord_id TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_players_discord ON players(discord_id)",
        [],
    )?;
    Ok(())
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

// ---- Discord identity -----------------------------------------------------------

/// The Discord id registered for an upload account, if any.
pub fn account_discord(conn: &Connection, account: &str) -> Option<String> {
    conn.query_row(
        "SELECT discord_id FROM accounts WHERE account = ?1 COLLATE NOCASE",
        [account],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .filter(|s| !s.is_empty())
}

pub fn set_account_discord(conn: &Connection, account: &str, discord_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO accounts (account, discord_id) VALUES (?1, ?2)
         ON CONFLICT(account) DO UPDATE SET discord_id = excluded.discord_id",
        params![account, discord_id],
    )?;
    Ok(())
}

/// Best-known Discord id for a player name: their account row, or any previous
/// match row already carrying one.
pub fn resolve_discord(conn: &Connection, name: &str) -> String {
    if let Some(id) = account_discord(conn, name) {
        return id;
    }
    conn.query_row(
        "SELECT discord_id FROM players
         WHERE name = ?1 COLLATE NOCASE AND discord_id != ''
         ORDER BY rowid DESC LIMIT 1",
        [name],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_default()
}

/// Backfill every row of `name` with a newly resolved Discord id.
pub fn backfill_discord(conn: &Connection, name: &str, discord_id: &str) -> rusqlite::Result<()> {
    if discord_id.is_empty() {
        return Ok(());
    }
    conn.execute(
        "UPDATE players SET discord_id = ?1 WHERE name = ?2 COLLATE NOCASE AND discord_id = ''",
        params![discord_id, name],
    )?;
    Ok(())
}

/// Called after a match insert: attach the uploader's Discord id to their player
/// row and resolve + backfill every other player in the match.
pub fn attach_discord_ids(conn: &Connection, uploader: &str, names: &[String]) -> rusqlite::Result<()> {
    let uploader_discord = account_discord(conn, uploader).unwrap_or_default();
    if !uploader_discord.is_empty() {
        backfill_discord(conn, uploader, &uploader_discord)?;
    }
    for name in names {
        let id = resolve_discord(conn, name);
        if !id.is_empty() {
            backfill_discord(conn, name, &id)?;
        }
    }
    Ok(())
}

// ---- Player stats --------------------------------------------------------------

/// Aggregate stats for one player identity (a Discord id, or a bare name when no
/// Discord id is known yet). The identity matches by `discord_id`, falling back to
/// an exact (case-insensitive) name match on rows with no Discord id.
pub fn player_stats(conn: &Connection, id: &str) -> rusqlite::Result<PlayerStats> {
    // Resolve the identity: a name with a known Discord id is queried by that id
    // (its backfilled rows carry it); a Discord id or an unresolved name queries
    // as-is. Rows that were never backfilled still match by bare name.
    let known = resolve_discord(conn, id);
    let discord_match = if known.is_empty() { id } else { &known };

    // The player's own rows: (match_uuid, result, race, name, discord_id).
    let mut stmt = conn.prepare(
        "SELECT p.match_uuid, p.result, p.race, p.name, p.discord_id
         FROM players p
         WHERE p.discord_id = ?1 COLLATE NOCASE
            OR (p.discord_id = '' AND p.name = ?2 COLLATE NOCASE)",
    )?;
    let rows = stmt.query_map(params![discord_match, id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;

    let mut match_uuids: Vec<String> = Vec::new();
    let mut games = 0i64;
    let mut wins = 0i64;
    let mut losses = 0i64;
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut discord_id = String::new();
    let mut own_race: std::collections::BTreeMap<String, (i64, i64)> = std::collections::BTreeMap::new();

    for row in rows {
        let (uuid, result, race, name, did) = row?;
        match_uuids.push(uuid.clone());
        games += 1;
        match result.as_str() {
            "won" => wins += 1,
            "lost" => losses += 1,
            _ => {}
        }
        names.insert(name);
        if !did.is_empty() {
            discord_id = did;
        }
        let (w, l) = own_race.entry(race).or_insert((0, 0));
        if result == "won" {
            *w += 1;
        } else if result == "lost" {
            *l += 1;
        }
    }

    // Opponent races: every other player row in the same matches.
    let mut vs_race: std::collections::BTreeMap<String, (i64, i64)> = std::collections::BTreeMap::new();
    let mut week_map: std::collections::BTreeMap<String, (i64, i64)> = std::collections::BTreeMap::new();
    let mut recent = Vec::new();
    for uuid in &match_uuids {
        let mut stmt = conn.prepare(
            "SELECT p.name, p.race, p.result FROM players p
             JOIN matches m ON m.uuid = p.match_uuid
             WHERE p.match_uuid = ?1",
        )?;
        let opp_rows = stmt.query_map([uuid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut my_result = "";
        for row in opp_rows {
            let (name, race, result) = row?;
            if names.contains(&name) {
                if result == "won" || result == "lost" {
                    my_result = if result == "won" { "won" } else { "lost" };
                }
                continue;
            }
            let (w, l) = vs_race.entry(race).or_insert((0, 0));
            match my_result {
                "won" => *w += 1,
                "lost" => *l += 1,
                _ => {}
            }
        }
        // Weekly progress.
        let week: String = conn.query_row(
            "SELECT strftime('%Y-W%W', uploaded_at) FROM matches WHERE uuid = ?1",
            [uuid],
            |r| r.get(0),
        )?;
        let (g, w) = week_map.entry(week).or_insert((0, 0));
        *g += 1;
        if my_result == "won" {
            *w += 1;
        }
    }

    // Recent matches (last 10).
    let mut stmt = conn.prepare(
        "SELECT m.uuid FROM matches m
         WHERE m.uuid IN (SELECT match_uuid FROM players WHERE discord_id = ?1 COLLATE NOCASE
                          OR (discord_id = '' AND name = ?2 COLLATE NOCASE))
         ORDER BY m.uploaded_at DESC LIMIT 10",
    )?;
    let uuids: Vec<String> = stmt
        .query_map(params![discord_match, id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for uuid in uuids {
        if let Some(summary) = match_summary(conn, &uuid)? {
            recent.push(summary);
        }
    }

    let to_race_stats = |m: std::collections::BTreeMap<String, (i64, i64)>| -> Vec<crate::model::RaceStat> {
        m.into_iter()
            .map(|(race, (w, l))| crate::model::RaceStat {
                race,
                games: w + l,
                wins: w,
                losses: l,
            })
            .collect()
    };

    Ok(PlayerStats {
        id: id.to_string(),
        names: names.into_iter().collect(),
        discord_id,
        games,
        wins,
        losses,
        vs_race: to_race_stats(vs_race),
        own_race: to_race_stats(own_race),
        weekly: week_map
            .into_iter()
            .map(|(week, (g, w))| crate::model::WeekStat { week, games: g, wins: w })
            .collect(),
        recent,
    })
}

/// A full match summary by uuid, or None.
pub fn match_summary(conn: &Connection, uuid: &str) -> rusqlite::Result<Option<MatchSummary>> {
    let row = conn
        .query_row(
            "SELECT uuid, game_version, map, game_type, duration_frames, matchup, uploaded_by, uploaded_at
             FROM matches WHERE uuid = ?1",
            [uuid],
            |r| {
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
            },
        )
        .optional()?;
    let Some((uuid, game_version, map, game_type, duration_frames, matchup, uploaded_by, uploaded_at)) = row else {
        return Ok(None);
    };
    let players = players_for(conn, &uuid)?;
    Ok(Some(MatchSummary {
        uuid,
        game_version,
        map,
        game_type,
        duration_frames,
        matchup,
        uploaded_by,
        uploaded_at,
        players,
    }))
}

pub fn players_for(conn: &Connection, uuid: &str) -> rusqlite::Result<Vec<PlayerSummary>> {
    let mut stmt = conn.prepare(
        "SELECT name, race, slot, result, discord_id FROM players WHERE match_uuid = ?1 ORDER BY slot",
    )?;
    let rows = stmt.query_map([uuid], |r| {
        Ok(PlayerSummary {
            name: r.get(0)?,
            race: r.get(1)?,
            slot: r.get(2)?,
            result: r.get(3)?,
            discord_id: r.get(4)?,
        })
    })?;
    rows.collect()
}
