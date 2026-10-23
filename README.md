# Replay Kraken (fork) — self-hosted Cosmonarchy replay archive

Your own Replay Kraken: the game client uploads replays automatically; this
server stores them, lets you browse/filter them (player, race, won/lost, date,
matchup), follow players, and announce results to a Discord channel via
webhook.

* **Phase 1 (this repo now):** upload API, filters, following, replay download,
  Discord result webhooks, minimal web UI, Docker/VPS-ready.
* **Phase 2 (next):** Discord OAuth login (accounts/follows tie to Discord),
  richer web UI.
* **Client:** the upload client lives in the Cosmonarchy gptp fork
  (`ReplayKraken.cpp`) — it is repointed at this server and extended with a
  metadata payload (players, races, result, map, matchup).

## Run

Bare (dev):

```sh
RK_TOKENS="you:secret" cargo run
# -> http://127.0.0.1:8787
```

Containerised (self-host or VPS):

```sh
cp .env.example .env   # then edit
docker compose up -d --build
```

## Client upload format

`POST /api/matches` with `Authorization: Bearer <token>`:

```json
{
  "matchUuid": "…",
  "gameVersion": "…",
  "replayFile": "<base64>",
  "desyncFile": "<base64>",
  "randomsFile": "<base64>",
  "meta": {
    "map": "The Samaritans",
    "gameType": "melee",
    "durationFrames": 12345,
    "matchup": "ZvT",
    "players": [
      { "name": "saintbellarmine", "race": "Zerg", "slot": 0, "result": "won" },
      { "name": "someone", "race": "Terran", "slot": 1, "result": "lost" }
    ]
  }
}
```

## API

| Method | Path | Notes |
|---|---|---|
| GET | `/api/health` | liveness |
| POST | `/api/matches` | upload (auth) |
| GET | `/api/matches` | list + filters: `player`, `race`, `result`, `matchup`, `from`, `to`, `followed_by` (`@me` resolves to the token's account), `limit`, `offset` |
| GET | `/api/matches/{uuid}/replay` | replay file |
| PUT/DELETE | `/api/players/{name}/follow` | follow / unfollow (auth) |
| GET | `/api/me/following` | your followed players (auth) |

Matchup filters are direction-insensitive (`ZvT` matches `TvZ`).

## Config

See `.env.example`. `RK_TOKENS` are simple bearer tokens (name:token pairs) —
Discord OAuth replaces them as the identity source in phase 2.
