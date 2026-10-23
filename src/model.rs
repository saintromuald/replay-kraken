//! Request/response models and matchup normalisation.

use serde::{Deserialize, Serialize};

/// The upload body sent by the client (gptp ReplayKraken protocol, extended with
/// `meta`). File fields are base64.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchUpload {
    pub match_uuid: Option<String>,
    pub game_version: Option<String>,
    pub replay_file: Option<String>,
    pub desync_file: Option<String>,
    pub randoms_file: Option<String>,
    /// Plus-fork metadata: players, map, matchup, result. Optional for
    /// compatibility with unmodified clients.
    pub meta: Option<MatchMeta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MatchMeta {
    pub map: Option<String>,
    pub game_type: Option<String>,
    pub duration_frames: Option<i64>,
    pub players: Vec<PlayerMeta>,
    pub matchup: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlayerMeta {
    pub name: String,
    pub race: String,
    pub slot: i64,
    pub result: Option<String>,
}

/// A match row as returned by list endpoints.
#[derive(Debug, Serialize)]
pub struct MatchSummary {
    pub uuid: String,
    pub game_version: String,
    pub map: String,
    pub game_type: String,
    pub duration_frames: i64,
    pub matchup: String,
    pub uploaded_by: String,
    pub uploaded_at: String,
    pub players: Vec<PlayerSummary>,
}

#[derive(Debug, Serialize)]
pub struct PlayerSummary {
    pub name: String,
    pub race: String,
    pub slot: i64,
    pub result: String,
}

/// List filters (`GET /api/matches` query parameters).
#[derive(Debug, Default, Clone)]
pub struct Filters {
    pub player: Option<String>,
    pub race: Option<String>,
    pub result: Option<String>,
    pub matchup: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub followed_by: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// Normalise a matchup string ("ZvT", "askosi v terran", ...) so both directions
/// match one filter: split on 'v', trim, sort the parts, rejoin.
pub fn normalize_matchup(m: &str) -> String {
    let mut parts: Vec<String> = m
        .split(|c| c == 'v' || c == 'V')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    parts.sort();
    parts.dedup();
    parts.join("v")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matchup_normalizes_directions() {
        assert_eq!(normalize_matchup("ZvT"), "tvz");
        assert_eq!(normalize_matchup("TvZ"), "tvz");
        assert_eq!(normalize_matchup("Askosi v Zerg"), "askosivzerg");
        assert_eq!(normalize_matchup("P v P"), "p");
        assert_eq!(normalize_matchup(""), "");
    }
}
