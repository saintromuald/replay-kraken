//! HTTP API: upload, list with filters, following, replay download, health.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;

use crate::db::{self, InsertMatch};
use crate::model::{Filters, MatchSummary, MatchUpload, PlayerSummary};
use crate::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/matches", post(upload_match).get(list_matches))
        .route("/api/matches/{uuid}/replay", get(replay_file))
        .route("/api/players/{name}/follow", put(follow).delete(unfollow))
        .route("/api/me/following", get(following))
        .route("/api/stats/{id}", get(player_stats))
        .route("/api/me/discord", post(claim_discord))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "replay-kraken" }))
}

/// Resolve the Bearer token to an account name, or 401.
fn authenticate(state: &AppState, headers: &axum::http::HeaderMap) -> Result<String, StatusCode> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .tokens
        .get(&auth)
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)
}

async fn upload_match(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<MatchUpload>,
) -> Response {
    let account = match authenticate(&state, &headers) {
        Ok(a) => a,
        Err(code) => return code.into_response(),
    };
    let uuid = body.match_uuid.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
        uuid::Uuid::new_v4().to_string()
    });
    if !uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return (StatusCode::BAD_REQUEST, "invalid matchUuid").into_response();
    }

    // Persist the uploaded files to data/replays/.
    let files: [(Option<&str>, &str); 3] = [
        (body.replay_file.as_deref(), crate::REPLAY_FILE_EXT),
        (body.desync_file.as_deref(), crate::DESYNC_FILE_EXT),
        (body.randoms_file.as_deref(), crate::RANDOMS_FILE_EXT),
    ];
    for (payload, ext) in files {
        let Some(payload) = payload else { continue };
        if payload.is_empty() {
            continue;
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload) else {
            return (StatusCode::BAD_REQUEST, format!("{ext} is not valid base64")).into_response();
        };
        let path = state.data_dir.join("replays").join(format!("{uuid}.{ext}"));
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("upload: cannot write {path:?}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot store replay",
            )
                .into_response();
        }
    }

    let meta = body.meta.unwrap_or_default();
    let matchup = meta.matchup.clone().unwrap_or_default();
    let matchup_norm = crate::model::normalize_matchup(&matchup);
    let record = InsertMatch {
        uuid: uuid.clone(),
        game_version: body.game_version.unwrap_or_default(),
        map: meta.map.unwrap_or_default(),
        game_type: meta.game_type.unwrap_or_default(),
        duration_frames: meta.duration_frames.unwrap_or(0),
        matchup,
        matchup_norm,
        uploaded_by: account.clone(),
        players: meta
            .players
            .into_iter()
            .map(|p| {
                (
                    p.name,
                    p.race,
                    p.slot,
                    p.result.unwrap_or_else(|| "unknown".to_string()),
                )
            })
            .collect(),
    };

    {
        let mut conn = state.db.lock().unwrap();
        if db::match_exists(&conn, &record.uuid).unwrap_or(false) {
            return (StatusCode::CONFLICT, "match already uploaded").into_response();
        }
        if let Err(e) = db::insert_match(&mut conn, &record) {
            eprintln!("upload: db error: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
        }
        // Attach Discord identities: the uploader's own id, plus every other
        // player's already-known id (backfilled across their history).
        let names: Vec<String> = record.players.iter().map(|(n, _, _, _)| n.clone()).collect();
        if let Err(e) = db::attach_discord_ids(&conn, &record.uploaded_by, &names) {
            eprintln!("upload: identity attach error: {e}");
        }
    }

    // Discord announcement (best-effort).
    if let Some(webhook) = &state.webhook_url {
        let summary = MatchSummary {
            uuid: uuid.clone(),
            game_version: record.game_version.clone(),
            map: record.map.clone(),
            game_type: record.game_type.clone(),
            duration_frames: record.duration_frames,
            matchup: record.matchup.clone(),
            uploaded_by: account,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
            players: record
                .players
                .iter()
                .map(|(name, race, slot, result)| PlayerSummary {
                    name: name.clone(),
                    race: race.clone(),
                    slot: *slot,
                    result: result.clone(),
                    discord_id: String::new(),
                })
                .collect(),
        };
        let webhook = webhook.clone();
        let base_url = state.base_url.clone();
        tokio::spawn(async move {
            crate::discord::announce(&webhook, &base_url, &summary).await;
        });
    }

    (StatusCode::CREATED, Json(serde_json::json!({ "uuid": uuid }))).into_response()
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    player: Option<String>,
    race: Option<String>,
    result: Option<String>,
    matchup: Option<String>,
    from: Option<String>,
    to: Option<String>,
    followed_by: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_matches(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    // `followed_by=@me` resolves to the account of the presented token.
    let followed_by = match q.followed_by.as_deref() {
        Some("@me") | Some("me") => match authenticate(&state, &headers) {
            Ok(account) => Some(account),
            Err(code) => return code.into_response(),
        },
        other => other.map(|s| s.to_string()),
    };
    let filters = Filters {
        player: q.player,
        race: q.race,
        result: q.result,
        matchup: q.matchup,
        from: q.from,
        to: q.to,
        followed_by,
        limit: q.limit.unwrap_or(50),
        offset: q.offset.unwrap_or(0),
    };
    let conn = state.db.lock().unwrap();
    match db::list_matches(&conn, &filters) {
        Ok((matches, total)) => Json(serde_json::json!({
            "matches": matches,
            "total": total,
            "limit": filters.limit,
            "offset": filters.offset,
        }))
        .into_response(),
        Err(e) => {
            eprintln!("list: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

async fn replay_file(
    State(state): State<Arc<AppState>>,
    Path(uuid): Path<String>,
) -> Response {
    if !uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state
        .data_dir
        .join("replays")
        .join(format!("{uuid}.{}", crate::REPLAY_FILE_EXT));
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            )],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn follow(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let account = match authenticate(&state, &headers) {
        Ok(a) => a,
        Err(code) => return code.into_response(),
    };
    let conn = state.db.lock().unwrap();
    match db::follow(&conn, &account, &name) {
        Ok(()) => Json(serde_json::json!({ "following": name })).into_response(),
        Err(e) => {
            eprintln!("follow: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

async fn unfollow(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let account = match authenticate(&state, &headers) {
        Ok(a) => a,
        Err(code) => return code.into_response(),
    };
    let conn = state.db.lock().unwrap();
    match db::unfollow(&conn, &account, &name) {
        Ok(()) => Json(serde_json::json!({ "unfollowed": name })).into_response(),
        Err(e) => {
            eprintln!("unfollow: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

async fn following(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let account = match authenticate(&state, &headers) {
        Ok(a) => a,
        Err(code) => return code.into_response(),
    };
    let conn = state.db.lock().unwrap();
    match db::following(&conn, &account) {
        Ok(players) => Json(serde_json::json!({ "players": players })).into_response(),
        Err(e) => {
            eprintln!("following: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

/// Stats for one player identity: a Discord id, or a bare player name when no
/// Discord id is known yet. Overall W/L, per-own-race and per-opponent-race
/// breakdowns, weekly progress, and the ten most recent matches.
async fn player_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if id.len() > 128 {
        return (StatusCode::BAD_REQUEST, "identity too long").into_response();
    }
    let conn = state.db.lock().unwrap();
    match db::player_stats(&conn, &id) {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => {
            eprintln!("stats: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct ClaimDiscord {
    #[serde(rename = "discordId")]
    discord_id: String,
}

/// Link the authenticated account to a Discord id (manually for now; the Discord
/// OAuth flow of phase 2 calls this after the token exchange).
async fn claim_discord(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ClaimDiscord>,
) -> Response {
    let account = match authenticate(&state, &headers) {
        Ok(a) => a,
        Err(code) => return code.into_response(),
    };
    let id = body.discord_id.trim().to_string();
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_digit()) {
        return (StatusCode::BAD_REQUEST, "invalid discord id").into_response();
    }
    let conn = state.db.lock().unwrap();
    if let Err(e) = db::set_account_discord(&conn, &account, &id) {
        eprintln!("claim: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response();
    }
    // Backfill the account's own history under the new identity.
    if let Err(e) = db::backfill_discord(&conn, &account, &id) {
        eprintln!("claim backfill: {e}");
    }
    Json(serde_json::json!({ "account": account, "discordId": id })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tower::ServiceExt as _;

    fn test_state() -> (Arc<AppState>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rk-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("replays")).unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let mut tokens = HashMap::new();
        tokens.insert("secret".to_string(), "alice".to_string());
        tokens.insert("secret2".to_string(), "bob".to_string());
        let state = Arc::new(AppState {
            db: Mutex::new(conn),
            tokens,
            data_dir: dir.clone(),
            webhook_url: None,
            base_url: "http://test".to_string(),
            public_read: true,
        });
        (state, dir)
    }

    fn json_body(v: serde_json::Value) -> String {
        serde_json::to_string(&v).unwrap()
    }

    async fn post(
        state: &Arc<AppState>,
        uri: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let app = router(state.clone());
        let mut req = Request::builder().method("POST").uri(uri);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let req = req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json_body(body)))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, value)
    }

    async fn get(
        state: &Arc<AppState>,
        uri: &str,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let app = router(state.clone());
        let mut req = Request::builder().method("GET").uri(uri);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, value)
    }

    fn match_body(uuid: &str) -> serde_json::Value {
        serde_json::json!({
            "matchUuid": uuid,
            "gameVersion": "861beb0",
            "replayFile": base64::engine::general_purpose::STANDARD.encode(b"replay-bytes"),
            "meta": {
                "map": "The Samaritans",
                "gameType": "melee",
                "durationFrames": 1200,
                "matchup": "ZvT",
                "players": [
                    { "name": "alice", "race": "Zerg", "slot": 0, "result": "won" },
                    { "name": "bob", "race": "Terran", "slot": 1, "result": "lost" }
                ]
            }
        })
    }

    #[tokio::test]
    async fn upload_requires_auth_and_dedupes() {
        let (state, dir) = test_state();
        let (status, _) = post(&state, "/api/matches", None, match_body("m1")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = post(&state, "/api/matches", Some("secret"), match_body("m1")).await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _) = post(&state, "/api/matches", Some("secret"), match_body("m1")).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(dir.join("replays/m1.rep").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn filters_player_race_result_and_matchup() {
        let (state, dir) = test_state();
        post(&state, "/api/matches", Some("secret"), match_body("m1")).await;
        post(
            &state,
            "/api/matches",
            Some("secret"),
            serde_json::json!({
                "matchUuid": "m2",
                "meta": {
                    "map": "Pagan Imperium",
                    "matchup": "TvZ",
                    "players": [
                        { "name": "carol", "race": "Terran", "slot": 0, "result": "lost" },
                        { "name": "dave", "race": "Zerg", "slot": 1, "result": "won" }
                    ]
                }
            }),
        )
        .await;

        let (_, v) = get(&state, "/api/matches?player=alice", None).await;
        assert_eq!(v["total"], 1);
        let (_, v) = get(&state, "/api/matches?race=Zerg", None).await;
        assert_eq!(v["total"], 2);
        let (_, v) = get(&state, "/api/matches?player=dave&result=won", None).await;
        assert_eq!(v["total"], 1);
        // Direction-insensitive matchup filter.
        let (_, v) = get(&state, "/api/matches?matchup=TvZ", None).await;
        assert_eq!(v["total"], 2);
        let (_, v) = get(&state, "/api/matches?matchup=ZvP", None).await;
        assert_eq!(v["total"], 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn follow_and_me_feed() {
        let (state, dir) = test_state();
        post(&state, "/api/matches", Some("secret"), match_body("m1")).await;
        post(
            &state,
            "/api/matches",
            Some("secret2"),
            serde_json::json!({
                "matchUuid": "m2",
                "meta": { "map": "X", "players": [ { "name": "carol", "race": "Zerg", "slot": 0, "result": "won" } ] }
            }),
        )
        .await;

        // alice follows carol.
        let (status, _) = put(&state, "/api/players/carol/follow", Some("secret")).await;
        assert_eq!(status, StatusCode::OK);
        let (_, v) = get(&state, "/api/me/following", Some("secret")).await;
        assert_eq!(v["players"], serde_json::json!(["carol"]));
        // The followed feed shows only carol's match.
        let (_, v) = get(&state, "/api/matches?followed_by=@me", Some("secret")).await;
        assert_eq!(v["total"], 1);
        assert_eq!(v["matches"][0]["uuid"], "m2");
        // Following without a token is refused.
        let (status, _) = get(&state, "/api/matches?followed_by=@me", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        std::fs::remove_dir_all(&dir).ok();
    }

    async fn put(state: &Arc<AppState>, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let app = router(state.clone());
        let mut req = Request::builder().method("PUT").uri(uri);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, value)
    }

    fn match_two(uuid: &str, a: (&str, &str, &str), b: (&str, &str, &str)) -> serde_json::Value {
        serde_json::json!({
            "matchUuid": uuid,
            "meta": {
                "map": "Map",
                "matchup": format!("{}v{}", a.1, b.1),
                "players": [
                    { "name": a.0, "race": a.1, "slot": 0, "result": a.2 },
                    { "name": b.0, "race": b.1, "slot": 1, "result": b.2 }
                ]
            }
        })
    }

    #[tokio::test]
    async fn player_stats_aggregate_and_resolve_discord_ids() {
        let (state, dir) = test_state();
        // Link alice's account to a Discord id, like RK_DISCORD_IDS / claim does.
        {
            let conn = state.db.lock().unwrap();
            db::set_account_discord(&conn, "alice", "111222").unwrap();
        }
        post(&state, "/api/matches", Some("secret"), match_two("s1", ("alice", "Zerg", "won"), ("bob", "Terran", "lost"))).await;
        post(&state, "/api/matches", Some("secret"), match_two("s2", ("alice", "Terran", "lost"), ("bob", "Protoss", "won"))).await;
        post(&state, "/api/matches", Some("secret"), match_two("s3", ("alice", "Zerg", "won"), ("carol", "Askosi", "lost"))).await;

        // Stats resolve by name...
        let (status, v) = get(&state, "/api/stats/alice", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["games"], 3);
        assert_eq!(v["wins"], 2);
        assert_eq!(v["losses"], 1);
        assert_eq!(v["discord_id"], "111222");
        // ... and by Discord id.
        let (_, v2) = get(&state, "/api/stats/111222", None).await;
        assert_eq!(v2["games"], 3);

        // vs-race: Terran 1-0, Protoss 0-1, Askosi 1-0.
        let vs = v["vs_race"].as_array().unwrap();
        let find = |race: &str| {
            vs.iter()
                .find(|r| r["race"] == race)
                .unwrap_or_else(|| panic!("missing vs {race}"))
                .clone()
        };
        assert_eq!(find("Terran")["wins"], 1);
        assert_eq!(find("Protoss")["losses"], 1);
        assert_eq!(find("Askosi")["wins"], 1);

        // own-race: Zerg 2-0, Terran 0-1.
        let own = v["own_race"].as_array().unwrap();
        let z = own.iter().find(|r| r["race"] == "Zerg").unwrap();
        assert_eq!((z["wins"].as_i64(), z["games"].as_i64()), (Some(2), Some(2)));

        // The opponent's stats work too (bob: 1-1).
        let (_, b) = get(&state, "/api/stats/bob", None).await;
        assert_eq!((b["wins"].as_i64(), b["losses"].as_i64()), (Some(1), Some(1)));

        std::fs::remove_dir_all(&dir).ok();
    }
}
