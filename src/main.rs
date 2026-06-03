use linemux::MuxedLines;
use regex::Regex;
use std::env;
use std::sync::OnceLock;
use tokio::sync::mpsc::{self};

use mc_rcon::RconClient;

use discord_bot::*;

use crate::discord_bot::dc_bot::*;

mod discord_bot;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (mc_event_tx, mc_event_rx) = mpsc::channel::<FromMinecraftEvent>(32);
    let (dc_event_tx, mut dc_event_rx) = mpsc::channel::<FromDiscordEvent>(32);

    tokio::spawn(async move {
        let mut lines_ok = match MuxedLines::new() {
            Ok(lines) => lines,
            Err(e) => {
                eprintln!("Failed to initialize MuxedLines background worker: {:?}", e);
                return;
            }
        };
        let path = env::var("LOG_PATH").unwrap_or_else(|_| {
            eprintln!("❌ Error: No LOG_PATH environment variable found!");
            std::process::exit(1);
        });
        println!("reading file {path:}");

        if let Err(why) = lines_ok.add_file(path.clone()).await {
            println!("failed to add file [{}] : {why:?}", path.clone())
        }

        while let Ok(Some(line)) = lines_ok.next_line().await {
            if let Some(event) = parse_log_line(line.line()) {
                let discord_payload = match event {
                    MinecraftEvent::Chat { username, message } => FromMinecraftEvent {
                        username,
                        content: message,
                    },
                    MinecraftEvent::Death { system_message } => {
                        let bold_msg = bold_first_word(&system_message);
                        FromMinecraftEvent {
                            username: "⚰️".to_string(),
                            content: bold_msg,
                        }
                    }
                    MinecraftEvent::Advancement { system_message } => {
                        let bold_msg = bold_first_word(&system_message);
                        FromMinecraftEvent {
                            username: "🏆".to_string(),
                            content: bold_msg,
                        }
                    }
                    MinecraftEvent::PlayerJoinLeave {
                        system_message,
                        is_join,
                    } => {
                        let icon = if is_join { "🟢 " } else { "🔴 " };
                        let bold_msg = bold_first_word(&system_message);
                        FromMinecraftEvent {
                            username: icon.to_string(),
                            content: bold_msg,
                        }
                    }
                };
                if let Err(why) = mc_event_tx.send(discord_payload).await {
                    println!("failed to send FromMinecraftEvent: {why:?}");
                }
            }
        }
    });

    tokio::spawn(async move {
        let rcon_server_address = env::var("RCON_SERVER_ADDRESS");
        let rcon_server_address = if let Ok(rcon_server_address_ok) = rcon_server_address {
            rcon_server_address_ok
        } else {
            println!(
                "RCON_SERVER_ADDRESS environment variable not detected, defaulting to localhost:25575"
            );
            "localhost:25575".to_string()
        };
        let Ok(rcon_client) = RconClient::connect(rcon_server_address) else {
            println!("unable to connect to minecraft rcon server");
            return;
        };
        let rcon_pass =
            env::var("RCON_PASSWORD").expect("Expected RCON_PASSWORD in the environment");
        if let Err(why) = rcon_client.log_in(&rcon_pass) {
            println!("failed to log in rcon server {why:?}")
        }

        while let Some(event) = dc_event_rx.recv().await {
            let formatted_command = format!(
                r#"tellraw @a {{"text":"[Discord] <{}>: {}", "color":"gold"}}"#,
                event.username, event.content
            );
            if let Err(why) = rcon_client.send_command(&formatted_command) {
                println!("failed to send command to rcon server: {why:?}")
            }
        }
    });

    start_dc_bot(mc_event_rx, dc_event_tx).await;
    Ok(())
}

fn parse_log_line(line: &str) -> Option<MinecraftEvent> {
    static CHAT_REGEX: OnceLock<Regex> = OnceLock::new();
    static SYSTEM_REGEX: OnceLock<Regex> = OnceLock::new();

    let chat_re = CHAT_REGEX.get_or_init(|| {
        Regex::new(
            r"^\[\d{2}:\d{2}:\d{2}\]\s\[[^\]]+/INFO\]:\s(?:\[Not Secure\]\s)?<(?P<username>[a-zA-Z0-9_]{3,16})>\s(?P<message>.+)$"
        ).unwrap()
    });

    let sys_re = SYSTEM_REGEX.get_or_init(|| {
        Regex::new(r"^\[\d{2}:\d{2}:\d{2}\]\s\[[^\]]+/INFO\]:\s(?P<payload>.+)$").unwrap()
    });

    // 1. Match Chat Events first
    if let Some(captures) = chat_re.captures(line) {
        let username = captures.name("username")?.as_str().to_string();
        let message = captures.name("message")?.as_str().to_string();
        return Some(MinecraftEvent::Chat { username, message });
    }

    // 2. Process System/Combat/Connection Lines
    if let Some(captures) = sys_re.captures(line) {
        let payload = captures.name("payload")?.as_str();

        // 3. Catch Player Join Events
        if payload.contains("joined the game") {
            return Some(MinecraftEvent::PlayerJoinLeave {
                system_message: payload.to_string(),
                is_join: true,
            });
        }

        // 4. Catch Player Leave / Disconnect Events
        if payload.contains("left the game") {
            return Some(MinecraftEvent::PlayerJoinLeave {
                system_message: payload.to_string(),
                is_join: false,
            });
        }

        if payload.contains("lost connection:") {
            return None;
        }

        if payload.contains("Logged in with entity id")
            || payload.contains("Saving chunks for level")
            || payload.contains("Stopping server")
            || payload.starts_with("Rcon connection from")
        {
            return None;
        }

        if payload.contains("has made the advancement")
            || payload.contains("has completed the challenge")
        {
            return Some(MinecraftEvent::Advancement {
                system_message: payload.to_string(),
            });
        }

        let is_death = payload.contains("was slain by")
            || payload.contains("was smashed by")
            || payload.contains("was impaled by")
            || payload.contains("was shot by")
            || payload.contains("was pummeled by")
            || payload.contains("was blown up by")
            || payload.contains("was skewered by")
            || payload.contains("was spit at by")
            || payload.contains("was struck by lightning")
            || payload.contains("was frozen to death")
            || payload.contains("was squashed by")
            || payload.contains("was squished too much")
            || payload.contains("was poked to death")
            || payload.contains("was pricked to death")
            || payload.contains("was doomed to fall")
            || payload.contains("fell from a high place")
            || payload.contains("hit the ground too hard")
            || payload.contains("fell out of the world")
            || payload.contains("didn't want to live")
            || payload.contains("experienced kinetic energy")
            || payload.contains("drowned")
            || payload.contains("suffocated in a wall")
            || payload.contains("starved to death")
            || payload.contains("burned to death")
            || payload.contains("went up in flames")
            || payload.contains("tried to swim in lava")
            || payload.contains("discovered the floor was lava")
            || payload.contains("withered away")
            || payload.contains("killed by magic")
            || payload.contains("left the confines of this world");

        if is_death {
            return Some(MinecraftEvent::Death {
                system_message: payload.to_string(),
            });
        }
    }

    None
}

fn bold_first_word(text: &str) -> String {
    if let Some((first_word, rest)) = text.split_once(' ') {
        format!("**{}** {}", first_word, rest)
    } else {
        format!("**{}**", text)
    }
}
