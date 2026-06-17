use std::sync::Arc;

use dotenvy::dotenv;
use linemux::MuxedLines;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use bot::types::{FromDiscordEvent, FromMinecraftEvent};

mod bot;
mod consts;
mod log_parser;
mod rcon;

#[tokio::main]
async fn main() -> Result<(), bot::BotError> {
    tracing_subscriber::fmt::init();
    dotenv().ok();

    let config = consts::Config::load().map_err(|e| {
        tracing::error!("Configuration error: {e}");
        bot::BotError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            e.to_string(),
        ))
    })?;

    let (mc_event_tx, mc_event_rx) = mpsc::channel::<FromMinecraftEvent>(32);
    let (dc_event_tx, mut dc_event_rx) = mpsc::channel::<FromDiscordEvent>(32);

    let log_path = config.log.path.clone();

    tokio::spawn(async move {
        let mut lines_ok = match MuxedLines::new() {
            Ok(lines) => lines,
            Err(e) => {
                tracing::error!("Failed to initialize MuxedLines background worker: {e:?}");
                return;
            }
        };

        tracing::info!("reading file {log_path}");

        if let Err(why) = lines_ok.add_file(log_path).await {
            tracing::warn!("failed to add log file: {why:?}");
        }

        while let Ok(Some(line)) = lines_ok.next_line().await {
            if let Some(event) = log_parser::parse_log_line(line.line()) {
                let discord_payload = FromMinecraftEvent::from(event);
                if let Err(why) = mc_event_tx.send(discord_payload).await {
                    tracing::warn!("failed to send FromMinecraftEvent: {why:?}");
                }
            }
        }
    });

    let rcon_client = rcon::connect(&config.rcon.address, &config.rcon.password)?;
    let shared_rcon = Arc::new(Mutex::new(rcon_client));
    let rcon_clone = Arc::clone(&shared_rcon);

    tokio::spawn(async move {
        while let Some(event) = dc_event_rx.recv().await {
            let formatted_command = format!(
                r#"tellraw @a {{"text":"[Discord] <{}>: {}", "color":"gold"}}"#,
                event.username, event.content
            );
            let guard = rcon_clone.lock().await;
            if let Err(why) = guard.send_command(&formatted_command) {
                tracing::warn!("failed to send command to rcon server: {why:?}");
            }
        }
    });

    bot::handler::start_bot(
        config.discord.token,
        config.bot.owner_id,
        config.minecraft.server_address,
        mc_event_rx,
        dc_event_tx,
        shared_rcon,
    )
    .await?;

    Ok(())
}
