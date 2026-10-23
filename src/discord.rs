//! Discord integration: result announcements via a channel webhook.
//! (OAuth login arrives in phase 2.)

use crate::model::MatchSummary;

/// Post a match result to the configured Discord channel webhook. Best-effort:
/// failures are logged, never fatal.
pub async fn announce(webhook_url: &str, base_url: &str, m: &MatchSummary) {
    let humans: Vec<&str> = m
        .players
        .iter()
        .filter(|p| p.result == "won" || p.result == "lost")
        .map(|p| p.name.as_str())
        .collect();
    let winners: Vec<String> = m
        .players
        .iter()
        .filter(|p| p.result == "won")
        .map(|p| format!("{} ({})", p.name, p.race))
        .collect();
    let summary = if winners.is_empty() {
        format!(
            "New replay: {} on {} ({}), players: {}",
            m.matchup,
            m.map,
            m.game_type,
            humans.join(" vs ")
        )
    } else {
        format!(
            "{} won on {} ({} vs {}) — type: {}",
            winners.join(", "),
            m.map,
            m.matchup,
            humans.join(" vs "),
            m.game_type
        )
    };
    let body = serde_json::json!({
        "username": "Replay Kraken",
        "content": format!("{summary}\n{base_url}/#/match/{}", m.uuid),
    });
    match reqwest::Client::new().post(webhook_url).json(&body).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                eprintln!("webhook: Discord returned {}", resp.status());
            }
        }
        Err(e) => eprintln!("webhook: {e}"),
    }
}
