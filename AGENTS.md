# VVV: Villager's Verse Viaduct — AI Agent Guide

> **AI Disclaimer:** This project is an AI-assisted fork of [OscillatingBlock's original Ruze](https://github.com/OscillatingBlock/Ruze), which was created as a toy learning project. The codebase has been heavily refactored and extended with new features using AI assistance (DeepSeek-V4-Pro and V4-Flash). The original author's work provided the foundation; all subsequent modifications and additions are AI-generated.

A lightweight, high-performance Discord–Minecraft chat bridge bot. Built in Rust with `poise`/`serenity` for Discord, tailing the Minecraft server's `latest.log` directly — no mods or plugins required.

---

## Table of Contents

- [Project Overview](#project-overview)
- [Architecture](#architecture)
- [Module Reference](#module-reference)
- [Data Flow](#data-flow)
- [Configuration](#configuration)
- [Build & Run](#build--run)
- [Testing](#testing)
- [Deployment](#deployment)
- [Key Conventions](#key-conventions)
- [Troubleshooting](#troubleshooting)

---

## Project Overview

VVV bridges Minecraft server chat to a Discord channel and vice versa. It reads the server's `latest.log` via `linemux` (file tailing), parses events (chat, join/leave, deaths, advancements, commands, server lifecycle), and forwards them into Discord as formatted messages. Messages from Discord are sent into Minecraft via RCON using `tellraw` JSON commands.

### Capabilities

| Feature | How |
|---|---|
| **Zero-Client Bridge** | Tails `latest.log` via `linemux` — no mods/plugins |
| **Live Chat Sync** | Bidirectional, with JSON-safe escaping |
| **Rich Server Events** | 95+ death message patterns, advancements, join/leave/disconnect, commands, server lifecycle |
| **Leave Deduplication** | Suppresses generic "left the game" when a disconnect reason is already known |
| **Persistent Bridge State** | Channel binding stored in `redb` database, survives restarts |
| **Player Info** | `/info` queries via RCON + Server List Ping (MOTD, latency, icon) |
| **Account Linking** | `/connect`/`/disconnect` links Discord ↔ Minecraft accounts |
| **Player Stats** | Play time, deaths, advancements, messages, commands — tracked per player |
| **Mention Cross-Translation** | `@DiscordUser` in MC → Discord ping; `@MCPlayer` in Discord → `@playername` in MC |
| **Privacy Controls** | `/unsub`/`/sub` for join/leave announcements, `/mutemention`/`/unmutemention` |
| **Admin Tools** | `/connect_admin` (owner only), `/mute`/`/unmute` (owner/admin) with duration support |
| **Structured Logging** | `tracing`-based with RFC 3339 timestamps, configurable verbosity |
| **Guild Welcome** | Themed embed on Discord member join |

---

## Architecture

```
┌───────────────┐     ┌─────────────────────────────────────────────────────┐
│  Minecraft    │────▶│  log_parser.rs   (linemux tails latest.log)          │
│  Server       │     │  └─ parse_log_line() → MinecraftEvent               │
│  latest.log   │     │     ├─ Chat / Join / Leave / Disconnect / Death     │
│               │     │     ├─ Advancement / Command / ServerSay             │
│  RCON         │◀────│     ├─ ServerStart / ServerStop / SaveComplete       │
│  (25575)      │     │     └─ UuidResolved / PlayerList                     │
│               │     │                                                      │
│  Server List  │◀────│  rcon.rs  (ReconnectingRcon — auto-reconnect)        │
│  Ping (25565) │     │                                                      │
└───────────────┘     │  stats.rs  (StatsTracker — write-coalescing)         │
                      │                                                      │
                      │  storage.rs  (redb persistence)                      │
                      │     ├─ bridge binding (channel ↔ guild)              │
                      │     ├─ username↔uuid mapping                         │
                      │     ├─ player stats (cumulative + daily)             │
                      │     ├─ dc↔mc account linking                         │
                      │     ├─ join/leave opt-out set                        │
                      │     ├─ mention mute set                              │
                      │     └─ muted users set (discord_id → expiry_ts)      │
                      │                                                      │
                      │  bot/  (poise/serenity Discord framework)             │
                      │     ├─ types.rs      — Data, events, formatting      │
                      │     ├─ handler.rs    — event_handler, forwarding     │
                      │     └─ commands.rs   — /ping, /info, /bridge, etc.  │
                      └─────────────────────────────────────────────────────┘
                                  │
                                  ▼
                        ┌────────────────────┐
                        │  Discord Gateway   │
                        └────────────────────┘
```

### Startup Sequence

1. **`main.rs`** — Initialize `tracing` subscriber, load configuration via `consts::Config::load()`.
2. **Channel setup** — Create 3 `mpsc` channels: `mc_event` (Minecraft→Discord), `dc_event` (Discord→Minecraft), `stats_event` (log parser→stats tracker).
3. **Log watcher** — Spawn a Tokio task that tails `latest.log` via `linemux::MuxedLines`, feeds each line into `log_parser::parse_log_line()`, then sends results to `mc_event_tx` and `stats_tx`.
4. **RCON** — Connect to Minecraft RCON (`ReconnectingRcon::connect`), wrap in `Arc`.
5. **DC→MC relay** — Spawn `spawn_dc_to_mc_relay` which reads from `dc_event_rx` and sends `tellraw` commands via RCON.
6. **Storage** — Open the `redb` database at `$XDG_STATE_HOME/vvv/vvv.redb`.
7. **Stats tracker** — Create `StatsTracker` with the stats channel receiver, spawn it as a background task.
8. **Bot** — Call `bot::handler::start_bot()` with all shared state; this builds the `poise` framework, registers slash commands, and connects to Discord.

### Channel Architecture

Three `mpsc` channels flow through the application:

| Channel | Type | Direction | Purpose |
|---|---|---|---|
| `mc_event_tx` → `mc_event_rx` | `FromMinecraftEvent` | Log parser → Bot handler | Minecraft chat/events forwarded to Discord |
| `dc_event_tx` → `dc_event_rx` | `FromDiscordEvent` | Bot handler → DC→MC relay | Discord messages relayed to Minecraft |
| `stats_tx` → `stats_rx` | `StatsEvent` | Log parser → Stats tracker | Lightweight event copy for stats recording |

---

## Module Reference

### `src/main.rs`

**Entry point and orchestration.** Sets up the Tokio runtime, initializes tracing, loads config, creates channels, spawns the log watcher task, connects RCON, spawns the DC→MC relay, opens storage, starts the stats tracker, and launches the Discord bot.

Key functions:
- `main()` — The async entry point. All setup happens here.
- `parse_mc_address(raw)` — Converts a config string into a `url::Url` with `mc://` scheme for the `rust-mc-status` client.
- `spawn_dc_to_mc_relay(rx, rcon)` — Background task that receives `FromDiscordEvent` values and sends them as `tellraw` JSON commands via RCON.

### `src/consts.rs`

**Configuration loading.** Implements a layered config system (XDG-based):

| Priority | Source | Path |
|---|---|---|
| 1 (lowest) | System-wide | `/etc/vvv.toml` |
| 2 | Per-user XDG | `$XDG_CONFIG_HOME/vvv.toml` (→ `~/.config/vvv.toml`) |
| 3 | Per-user home | `$HOME/.vvv.toml` |
| 4 (highest) | Environment | `VVV_*` variables (with deprecated old-name fallback) |

Key types:
- `Config` — Top-level config struct with sub-configs: `DiscordConfig`, `RconConfig`, `MinecraftConfig`, `BotConfig`, `LogConfig`, `StorageConfig`, `StatsConfig`.
- `ConfigError` — Error enum with `MissingField`, `ReadFile`, `ParseToml` variants.

Key functions:
- `Config::load()` — Loads, merges, validates. Returns `Config`.
- `Config::merge_file()` — Reads a TOML file and merges into config.
- `Config::merge_into()` — Field-by-field merge (non-empty fields overwrite).
- `Config::overlay_env()` — Reads `VVV_*` env vars (and deprecated fallbacks).
- `Config::validate()` — Ensures required fields are present.
- `parse_timezone(tz_str)` — Parses IANA timezone string, falls back to UTC.
- `resolve_db_path(config)` — Resolves redb path from config or default.
- `default_db_path()` — Returns `$XDG_STATE_HOME/vvv/vvv.redb`.

### `src/log_parser.rs`

**Minecraft log line parser.** The core parsing engine. Uses `regex` for chat lines and string matching for system events. No external dependencies beyond `regex`.

Key types:
- `MinecraftEvent` — Enum with variants: `Chat`, `Join`, `Leave`, `Disconnect`, `Death`, `Advancement`, `Command`, `ServerSay`, `PlayerList`, `ServerStart`, `ServerStop`, `SaveComplete`, `UuidResolved`.
- `StatsEvent` — Lightweight copy of events relevant to stats tracking (borrows from `MinecraftEvent`).

Key functions:
- `parse_log_line(line)` — Main entry point. Tries chat first, then server say, then system payload extraction. Returns `Option<MinecraftEvent>`.
- `MinecraftEvent::to_stats_event(&self)` — Extracts a `StatsEvent` by borrowing `&self`, called *before* `into_discord()`.
- `is_silent_message_prefix(text)` — DC→MC: checks if text starts with `@silent` or `@s`.
- `contains_silent_token(text)` — MC→DC: checks if `@s` appears as a standalone token.

Static data:
- `DEATH_PATTERNS` — 43 death message substrings (e.g. "was slain by", "drowned", "fell from a high place").
- `IGNORE_PATTERNS` — Lines to skip (RCON commands, AuthMe, chunk saves, etc.).
- `PRIVATE_COMMAND_PATTERNS` — Commands not forwarded (`/msg`, `/tell`, `/whisper`, etc.).
- `RECENT_DISCONNECTS` — Global `Mutex<HashMap<String, String>>` tracking recent disconnects for deduplication.
- `LAST_SERVER_STOP` — Global `Mutex<Option<Instant>>` for deduplicating server-stop events.

Parsing strategy:
1. `try_chat()` — Regex on `[HH:MM:SS] [thread/INFO]: <username> message`.
2. `try_server_say()` — Matches `[HH:MM:SS] [thread/INFO]: [Server] message`.
3. `extract_system_payload()` — Strips timestamp and thread prefix to get the raw payload.
4. `try_death()`, `try_join()`, `try_leave()`, `try_disconnect()`, `try_command()`, `try_advancement()`, `try_player_list()`, `try_server_start()`, `try_server_stop()`, `try_save_complete()`, `try_uuid()` — Each checks the payload against known patterns.

### `src/rcon.rs`

**Minecraft RCON client with auto-reconnect.** Wraps the `mc-rcon` crate's synchronous `RconClient` in an async-friendly `Arc<ReconnectingRcon>`.

Key types:
- `RconError` — `Connect` (I/O error) or `Rcon` (protocol error string).
- `ReconnectingRcon` — Holds `address`, `password`, `Mutex<Option<RconClient>>`, `Mutex<Option<Instant>>` for rate-limiting.

Key functions:
- `ReconnectingRcon::connect(address, password)` — Initial connect, returns `Result<Self, RconError>`.
- `ReconnectingRcon::send_command(self: &Arc<Self>, command)` — Async method that wraps `send_command_sync` in `spawn_blocking`. On failure, reconnects and retries.
- `send_command_sync(command)` — Attempts send, reconnects on failure with rate-limiting (5s cooldown).
- `create_client_with_timeout()` — Spawns a detached OS thread with `recv_timeout` (3s) for bounded connect.

### `src/stats.rs`

**Player statistics tracker.** Background task that consumes `StatsEvent` values from the log parser, accumulates in-memory deltas, and flushes to `redb` periodically.

Key types:
- `StatsTracker` — Holds `Arc<Storage>`, `Receiver<StatsEvent>`, `Arc<ReconnectingRcon>`, `Tz` (timezone), `PendingState`.
- `PendingState` — In-memory accumulators: `uuid_cache`, `online_sessions` (username→login timestamp), `pending_deltas` (username→`PlayerDelta`), `pending_daily` (combined key→seconds).

Key functions:
- `StatsTracker::run()` — Event loop with `tokio::select!` on `stats_rx.recv()` and a 60s flush timer.
- `StatsTracker::handle_event()` — Routes stats events to the appropriate accumulator.
- `StatsTracker::handle_join()` — Records session start, checks if player is new, sends welcome/login-reminder `tellraw`.
- `StatsTracker::handle_leave()` — Computes session duration, splits by calendar day, flushes immediately.
- `StatsTracker::flush_all_sessions()` — On server stop, forces all online sessions to be recorded.
- `StatsTracker::flush(durability)` — Drains pending deltas/daily splits into storage. Uses `Durability::None` (no fsync) for periodic flushes, `Durability::Immediate` for leave/stop.
- `split_session_by_day(start_ts, end_ts, tz)` — Splits a session into per-day buckets by calendar day in the configured timezone.
- `format_duration(secs)` — Formats seconds as `"Xd Yh Zm"` (omits zero components).

### `src/storage.rs`

**Persistence layer.** Uses `redb` (embedded key-value store, similar to LMDB/RocksDB) for all durable state. Small lookup tables are cached in-memory behind `RwLock`s for zero-overhead reads.

Table definitions:
| Constant | Key | Value | Purpose |
|---|---|---|---|
| `BRIDGE` | `&str` ("current") | `(channel_id, guild_id, mc_server_address)` | Active bridge binding |
| `USERNAME_UUID` | `String` (username) | `String` (uuid) | Username→UUID mapping |
| `PLAYERS` | `String` (uuid) | `(play_time, first_login, last_login, last_logout, logins, deaths, advancements, messages, commands)` | Cumulative player stats |
| `DC_TO_MC` | `u64` (discord_id) | `String` (mc_username) | Discord→Minecraft account link |
| `MC_TO_DC` | `String` (mc_username) | `u64` (discord_id) | Minecraft→Discord reverse link |
| `JOIN_LEAVE_OPTOUT` | `u64` (discord_id) | `bool` | Opted out of join/leave announcements |
| `MUTE_MENTION` | `u64` (discord_id) | `bool` | Muted cross-chat mentions |
| `MUTED_USERS` | `u64` (discord_id) | `u64` (expiry_ts) | Muted from sending bridge messages |
| `SETTINGS` | `&str` ("privacy_enabled") | `bool` | Global privacy toggle |
| `DAILY_PLAY_TIME` | `(String, String)` (uuid, date) | `u64` (secs) | Daily play time splits |

Key types:
- `Storage` — Cloneable handle holding `Arc<Database>` + `Arc<RwLock<...>>` for each cache.
- `PlayerStats` — Struct with all stat fields, with `from_tuple`/`to_tuple` conversions.
- `PlayerDelta` — Per-player delta for write-coalescing.
- `DailyTimeSplit` — `(date, secs)` pair for daily play time.
- `StorageError` — Wraps all `redb` error variants plus `AlreadyClaimed` and `BlockingPanic`.

Key functions:
- `Storage::open(path, mc_server_address)` — Opens/creates database, loads in-memory caches.
- `get_bridge_channel()` / `set_bridge_channel()` / `clear_bridge_channel()` — Bridge binding CRUD.
- `store_uuid_mapping(username, uuid)` / `get_mc_uuid(username)` — UUID resolution.
- `get_player_stats(uuid)` / `flush_player_deltas()` — Stats read/write.
- `get_daily_play_time(uuid, date)` / `store_daily_play_time()` — Daily play time.
- `connect_accounts(discord_id, mc_username)` / `remove_connection(discord_id)` — Account linking.
- `get_mc_from_dc(discord_id)` / `get_dc_from_mc(mc_username)` — Lookups via cached binary search.
- `is_connected_dc(discord_id)` / `is_connected_mc(mc_username)` — Existence checks.
- `set_join_leave_optout(discord_id, opted_out)` / `is_join_leave_optout(discord_id)` — Opt-out.
- `set_mention_mute(discord_id, muted)` / `is_mention_muted(discord_id)` — Mute.
- `set_muted(discord_id, duration_secs)` / `is_muted(discord_id)` / `unmute_user(discord_id)` — Bridge mute management.
- `set_privacy_enabled(enabled)` / `is_privacy_enabled()` — Global toggle.
- `get_leaderboard()` — Returns top players by play time.

All `redb` operations run inside `spawn_blocking` because `redb` transactions are not `Send`.

### `src/bot/mod.rs`

**Bot module root.** Re-exports `commands`, `handler`, `types` submodules. Defines:

- `BotError` — Unified error type covering Serenity, I/O, Config, RCON, and Storage errors. Implements `From<poise::serenity_prelude::Error>` via `Box`.
- `Context<'a>` — Type alias for `poise::Context<'a, types::Data, BotError>`.

### `src/bot/types.rs`

**Data types and event formatting.** Defines the shared state struct and Minecraft→Discord event formatting.

Key types:
- `FromMinecraftEvent` — `{ username, content, mc_username }` — What the log parser sends to the Discord handler.
- `FromDiscordEvent` — `{ username, content }` — What the Discord handler sends to the RCON relay.
- `PendingVerification` — `{ discord_user_id, mc_username, expires_at, attempts }` — For `/connect` verification flow.
- `BotParams` — `{ token, owner_id, guild_id }` — Startup config consumed once.
- `Data` — The `poise` user data struct, shared across all commands via `ctx.data()`:
  - `dc_event_tx` — Sender for Discord→MC messages.
  - `mc_status_client` — `McClient` for Server List Ping.
  - `bridge_channel` — `Arc<RwLock<Option<ChannelId>>>` — Active bridge binding.
  - `storage` — `Arc<Storage>`.
  - `rcon_client` — `Arc<ReconnectingRcon>`.
  - `mc_server_address` — `url::Url`.
  - `pending_verifications` — `Arc<Mutex<HashMap<String, PendingVerification>>>`.

Key functions:
- `MinecraftEvent::into_discord(self)` — Converts a parsed `MinecraftEvent` into `Option<FromMinecraftEvent>` with emoji-prefixed, markdown-formatted content. Maps all variants except `PlayerList` and `UuidResolved` (returns `None` for those).

### `src/bot/handler.rs`

**Event handler and message forwarding.** The heart of the Discord integration.

Key functions:
- `start_bot(params, mc_server_address, mc_event_rx, dc_event_tx, rcon_client, storage)` — Builds the `poise` framework, registers commands, starts the Discord client. This is called from `main.rs`.
- `event_handler(ctx, event, data)` — Serenity event handler dispatched by the framework. Handles:
  - `Message` — Processes incoming Discord messages: checks they're in the bridged channel, checks mute status, processes mentions, checks silent prefixes, forwards to `dc_event_tx`.
  - `GuildMemberAddition` — Sends a themed welcome embed to the guild's system channel.
- `forward_mc_events(ctx, data, mc_event_rx, bridge_channel, storage)` — Background task spawned inside `start_bot`. Reads from `mc_event_rx` and sends messages to the bridged Discord channel. Handles:
  - Mention cross-translation (MC→DC).
  - Join/leave opt-out filtering.
  - Silent message suppression (`@s` token).
  - Verification code detection (for `/connect` flow).
- `process_dc_mentions(content, storage)` — Replaces `<@DISCORD_ID>` with `@mc_username` for Discord→MC messages, using the stored account link cache.
- `process_mc_mentions(content, sender_mc, storage)` — Replaces MC usernames with `<@DISCORD_ID>` pings for MC→DC messages, respecting mute preferences.

### `src/bot/commands.rs`

**All slash and prefix commands.** Registered in `handler.rs` via `poise::FrameworkOptions::commands`.

| Command | Access | Description |
|---|---|---|
| `~ping` | Everyone | Liveness check |
| `~info` (aliases: `players`, `now_playing`, `online_players`) | Everyone | RCON `list` + Server List Ping → embed with MOTD, latency, players, icon |
| `~start_bridge` | Owner/Admin | Bind bridge to current channel |
| `~stop_bridge` | Owner/Admin | Unbind bridge from current channel |
| `~stats` | Everyone | Player stats embed (play time, deaths, advancements, messages, commands) |
| `~playtime` | Everyone | Daily play time breakdown |
| `~leaderboard` | Everyone | Top players by play time |
| `~connect <mc_username>` | Everyone | Link Discord→MC account with in-game verification code |
| `~connect_admin <mc_username> <@user>` | Owner | Manually link accounts (bypasses verification) |
| `~disconnect` | Everyone | Unlink Discord→MC account |
| `~sub` | Everyone | Opt in to join/leave announcements |
| `~unsub` | Everyone | Opt out of join/leave announcements |
| `~mutemention` | Everyone | Mute cross-chat mentions |
| `~unmutemention` | Everyone | Unmute cross-chat mentions |
| `~mute <user> [duration]` | Owner/Admin | Mute user from bridge (default 5m, supports s/m/h/d) |
| `~unmute <user>` | Owner/Admin | Remove a bridge mute |
| `~privacy` | Owner | Toggle global privacy feature |
| `~help` | Everyone | List commands or detailed help |

Key helper functions:
- `url_to_hostport(url)` — Extracts `host:port` from `url::Url`.
- `parse_player_list(response)` — Parses RCON `list` output into player names.
- `generate_verification_code()` — 6-character alphanumeric code for `/connect` verification.
- `is_valid_mc_username(name)` — Validates 3–16 chars, alphanumeric + underscore.
- `parse_duration(input)` — Parses `"5m"`, `"1h"`, `"1d"`, `"30s"` into seconds.

---

## Data Flow

### Minecraft → Discord

```
latest.log line
  → log_parser::parse_log_line()
    → MinecraftEvent::to_stats_event()  → stats_tx → StatsTracker
    → MinecraftEvent::into_discord()    → mc_event_tx
      → handler::forward_mc_events()
        → process_mc_mentions() (MC→DC mention translation)
        → ctx.channel_id().send_message()  → Discord channel
```

### Discord → Minecraft

```
Discord message
  → event_handler (Message event)
    → mute check (is_muted)
    → privacy check (is_connected_dc, is_join_leave_opted_out)
    → is_silent_message_prefix() check
    → process_dc_mentions() (DC→MC mention translation)
    → dc_event_tx
      → spawn_dc_to_mc_relay()
        → rcon.send_command(tellraw JSON)  → Minecraft server
```

### Verification Flow

```
/connect <mc_username>
  → generates 6-char code, stores PendingVerification with 30s expiry
  → "Type @s CONFIRM-<code> in Minecraft chat"

Minecraft chat "@s CONFIRM-<code>"
  → log_parser detects CONFIRM- prefix (bypasses silent filter)
  → handler::forward_mc_events() detects CONFIRM- in content
    → matches against pending_verifications
    → storage::set_connection(discord_id, mc_username)
    → "✅ Verified!" message in Discord
```

---

## Configuration

### TOML file (`vvv.toml`)

```toml
[discord]
token = "your_discord_bot_token"

[rcon]
address = "localhost:25575"       # default
password = "your_rcon_password"

[minecraft]
server_address = "localhost:25565" # default

[bot]
owner_id = 123456789012345678
# guild_id = 123456789012345678    # optional: instant slash cmd sync

[log]
path = "/var/minecraft/logs/latest.log"

[storage]
# database_path = "/custom/path/vvv.redb"   # optional override

[stats]
# timezone = "UTC"                           # optional, default UTC
```

### Environment variables

| Variable | Required | Default |
|---|---|---|
| `VVV_DISCORD_TOKEN` | Yes | — |
| `VVV_LOG_PATH` | Yes | — |
| `VVV_RCON_PASSWORD` | Yes | — |
| `VVV_RCON_ADDRESS` | No | `localhost:25575` |
| `VVV_MC_SERVER_ADDRESS` | No | `localhost:25565` |
| `VVV_OWNER_ID` | Yes | — |
| `VVV_GUILD_ID` | No | — |
| `VVV_DATABASE_PATH` | No | `$XDG_STATE_HOME/vvv/vvv.redb` |

Deprecated variable names (`DISCORD_TOKEN`, `LOG_PATH`, `RCON_PASSWORD`, `RCON_SERVER_ADDRESS`, `MC_SERVER_QUERY_ADDRESS`) are still detected but log a warning.

### Minecraft server requirements

In `server.properties`:
```properties
enable-rcon=true
rcon.port=25575
rcon.password=your_secure_rcon_password_here
```

Discord: Enable **Server Members Intent** and **Message Content Intent** in the Developer Portal.

---

## Build & Run

```bash
# Debug
cargo run

# Release (recommended for production)
cargo run --release

# Verbose logging
RUST_LOG=debug cargo run --release

# Quiet noisy crates
RUST_LOG=info,serenity=warn,tracing=warn cargo run --release
```

### Requirements

- Rust edition 2024 (MSRV determined by `poise`/`serenity` — stable Rust should work)
- Tokio multi-threaded runtime
- Access to a Minecraft server's `latest.log` and RCON port

### Log format

Structured JSON-like output with RFC 3339 timestamps, file location, and structured fields:

```
2026-06-17T10:30:00.123Z  INFO main.rs:18 starting VVV bridge...
2026-06-17T10:30:00.125Z  INFO consts.rs:88 loading configuration...
2026-06-17T10:30:00.132Z  INFO rcon.rs:16 RCON connected to localhost:25575
```

---

## Testing

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Specific test
cargo test test_name
```

Test files are inline in each module (no separate `tests/` directory). Notable test modules:
- `log_parser.rs` — 30+ tests covering all event types, edge cases, death message patterns, modded entity deaths, and the `CONFIRM-` code path.
- `stats.rs` — Tests for `split_session_by_day` (UTC, IST, multi-day) and `format_duration`.
- `storage.rs` — Integration tests using temporary redb files for bridge CRUD, account linking, muted users, and parent directory creation.

---

## Deployment

### Using systemd (recommended)

```ini
[Unit]
Description=VVV Discord-Minecraft Bridge
After=network.target minecraft.service

[Service]
Type=simple
User=minecraft
Environment=VVV_DISCORD_TOKEN=your_token
Environment=VVV_LOG_PATH=/var/minecraft/logs/latest.log
Environment=VVV_RCON_PASSWORD=your_password
Environment=VVV_OWNER_ID=123456789012345678
ExecStart=/usr/local/bin/vvv
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

### Using Docker

```dockerfile
FROM rust:alpine AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM alpine:latest
COPY --from=builder /app/target/release/vvv /usr/local/bin/vvv
CMD ["vvv"]
```

### Data layout

| Path | Purpose |
|---|---|
| `$XDG_CONFIG_HOME/vvv/vvv.toml` | User configuration |
| `$XDG_STATE_HOME/vvv/vvv.redb` | redb database (bridge state, accounts, stats) |

---

## Key Conventions

### Code style

- **Edition 2024** — Uses `let Some(x) = expr` assignment-in-if (feature stabilized in Rust 1.82+).
- **Clippy** — `deny` on all lints, `warn` on pedantic. `nonstandard_style` is `deny`.
- **Release profile** — `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`.
- **Error handling** — `thiserror` for all error enums. `BotError` unifies all error types via `From` impls.
- **Logging** — `tracing` throughout. Use `tracing::info!`, `tracing::warn!`, `tracing::debug!`, `tracing::error!` with structured fields.
- **No unsafe code** — The project uses zero `unsafe` blocks.
- **Dead code allowed** — `#[allow(dead_code)]` is used sparingly on enum variants that may be unused in some builds.

### Project conventions

- **No mods/plugins** — The bridge is intentionally agent-side. All data comes from log files and RCON.
- **Async everywhere** — Tokio multi-threaded runtime. RCON wraps blocking I/O in `spawn_blocking`.
- **Write-coalescing** — Stats tracker accumulates in memory and flushes every 60s (or immediately on leave/stop).
- **In-memory caches** — Storage keeps small lookup tables in `RwLock`-guarded memory for zero-overhead reads.
- **Leave deduplication** — `RECENT_DISCONNECTS` global store suppresses "left the game" when a disconnect reason was already logged.
- **Verification codes** — In-game `@s CONFIRM-<code>` verification bypasses the `@s` silent filter in `log_parser.rs`.
- **Mute auto-cleanup** — `is_muted()` automatically removes expired mute entries from the in-memory cache and redb.

### Editing guidelines

1. **Always run `cargo test` and `cargo clippy`** before marking changes complete. The project has `deny` on all clippy lints.
2. **Follow the existing error pattern** — `thiserror` derives, `From` impls, and `BotError` unification.
3. **Keep log parsing stateless** — `parse_log_line` is a pure function. The only global state is `RECENT_DISCONNECTS` and `LAST_SERVER_STOP` for deduplication.
4. **New commands** — Add to `commands.rs` and register in `handler.rs`'s `vec![...]` in `FrameworkOptions::commands`.
5. **New tables** — Add `TableDefinition` constants in `storage.rs`, implement accessor methods, and wire up in-memory caches.
6. **Configuration** — Add fields to the appropriate `*Config` struct in `consts.rs`, handle merging in `merge_into` and `overlay_env`, and add validation in `validate`.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Bot doesn't start | Missing config field | Check `VVV_*` env vars or config file |
| "RCON connection failed" | Wrong address/password, or firewall | Verify `server.properties` and network |
| No events in Discord | Bridge not started | Run `~start_bridge` in the target channel |
| Duplicate "left the game" | Leave dedup not working | Check `RECENT_DISCONNECTS` in `log_parser.rs` |
| Stats not recording | Stats channel closed | Check `stats_tx` in `main.rs` |
| Slash commands not appearing | Global sync delay | Set `guild_id` in config for instant registration |
| `/connect` fails | Already linked | Use `/disconnect` first |
| Mentions not working | Account not linked or mention muted | Use `/connect` and check `/mutemention` |

---

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `tokio` | 1 | Async runtime (multi-thread, macros, sync) |
| `poise` | 0.6 | Discord command framework (serenity wrapper) |
| `serenity` | (via poise) | Discord API client |
| `linemux` | 0.3 | File tailing (inotify/kqueue/poll) |
| `regex` | 1.12 | Log line parsing |
| `mc-rcon` | 0.1 | Minecraft RCON protocol |
| `rust-mc-status` | 3.0 | Server List Ping (MOTD, latency, icon) |
| `redb` | 4.1 | Embedded key-value store (persistence) |
| `chrono` + `chrono-tz` | 0.4 / 0.10 | Timezone-aware time handling |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | Structured logging |
| `serde` + `toml` | 1 / 1.1 | Config serialization |
| `thiserror` | 2 | Error derive macros |
| `base64` | 0.22 | Server icon decoding |
| `url` | 2 | Minecraft address parsing |
| `rand` | 0.10 | Verification code generation |