use mc_rcon::RconClient;
use poise::serenity_prelude as serenity;
use rust_mc_status::{McClient, ServerData, ServerEdition};
use serenity::all::Mentionable;
use std::{collections::HashSet, env, sync::Arc, time::Duration};
use tokio::sync::{
    Mutex, RwLock,
    mpsc::{Receiver, Sender},
};

use crate::discord_bot::{FromDiscordEvent, FromMinecraftEvent};

type Context<'a> = poise::Context<'a, Data, Error>;
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct Data {
    pub dc_event_tx: Sender<FromDiscordEvent>,
    pub mc_status_client: McClient,
    pub target_channel_id_list: Arc<RwLock<Option<Vec<serenity::ChannelId>>>>,
    pub rcon_client: Arc<Mutex<RconClient>>,
}

// Hardcoded user verification gate
const OWNER_ID: u64 = 1314616785156444175;

pub async fn start_dc_bot(
    mut mc_event_rx: Receiver<FromMinecraftEvent>,
    dc_event_tx: Sender<FromDiscordEvent>,
    rcon_client: Arc<Mutex<RconClient>>,
) {
    let token = env::var("DISCORD_TOKEN").expect("missing DISCORD_TOKEN");
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let bridge_channel_list = Arc::new(RwLock::new(None));
    let bridge_channel_list_clone = Arc::clone(&bridge_channel_list);

    let client = McClient::new()
        .with_timeout(Duration::from_secs(5))
        .with_max_parallel(10);

    let mut owners = HashSet::new();
    owners.insert(serenity::UserId::new(OWNER_ID));

    let framework: poise::Framework<Data, Error> = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            event_handler: |ctx, event, _, data| Box::pin(event_handler(ctx, event, data)),
            commands: vec![ping(), start_bridge(), stop_bridge(), info(), help()],
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some("~".into()),
                edit_tracker: Some(Arc::new(poise::EditTracker::for_timespan(
                    Duration::from_secs(3600),
                ))),
                additional_prefixes: vec![
                    poise::Prefix::Literal("hey reze,"),
                    poise::Prefix::Literal("hey reze"),
                ],
                ..Default::default()
            },
            owners,
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                // Register slash commands globally instantly on login setup
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;

                Ok(Data {
                    dc_event_tx,
                    mc_status_client: client,
                    target_channel_id_list: bridge_channel_list.clone(),
                    rcon_client,
                })
            })
        })
        .build();

    let client_builder = serenity::ClientBuilder::new(token, intents).framework(framework);
    let mut client = client_builder.await.unwrap();
    let cache_http = Arc::clone(&client.http);

    tokio::spawn(async move {
        while let Some(event) = mc_event_rx.recv().await {
            let formatted_message = format!("**{}**: {}", event.username, event.content);

            let current_targets = {
                let lock = bridge_channel_list_clone.read().await;
                lock.clone()
            };

            if let Some(target_channels) = current_targets {
                for target_channel in target_channels {
                    let http_clone = Arc::clone(&cache_http);
                    let msg_clone = formatted_message.clone();

                    tokio::spawn(async move {
                        if let Err(why) = target_channel.say(http_clone, msg_clone).await {
                            println!(
                                "failed to send message to discord channel {target_channel}: {why:?}"
                            );
                        }
                    });
                }
            }
        }
    });

    client.start().await.unwrap();
}

async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot, .. } => {
            println!("Logged in as {}", data_about_bot.user.name)
        }
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            let Some(system_channel) = new_member
                .guild_id
                .to_guild_cached(ctx)
                .and_then(|g| g.system_channel_id)
            else {
                return Ok(());
            };

            let welcome_embed = serenity::CreateEmbed::new()
                .title("💥 A New Target Approaches! 💥")
                .description(format!(
                    "Welcome to the server, {}! Let's hope things don't get too... explosive. 🤫",
                    new_member.mention()
                ))
                .color(0x9b59b6)
                .thumbnail(
                    "https://i.pinimg.com/originals/5d/15/4b/5d154b68de57a87600fe9b98d692802c.gif",
                )
                .footer(serenity::CreateEmbedFooter::new(format!(
                    "Member Count: #{}",
                    new_member
                        .guild_id
                        .to_guild_cached(ctx)
                        .map(|g| g.member_count)
                        .unwrap_or(0)
                )));

            let message = serenity::CreateMessage::new().embed(welcome_embed);
            let _ = system_channel.send_message(&ctx.http, message).await;
        }
        serenity::FullEvent::Message { new_message } => {
            let current_targets = {
                let lock = data.target_channel_id_list.read().await;
                lock.clone()
            };

            if let Some(target_channels) = current_targets {
                if target_channels.contains(&new_message.channel_id)
                    && new_message.author.id != ctx.cache.current_user().id
                {
                    let _ = data
                        .dc_event_tx
                        .send(FromDiscordEvent {
                            username: new_message.author.name.clone(),
                            content: new_message.content.clone(),
                        })
                        .await;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn ping_help() -> String {
    String::from("Use this to check if I'm alive!")
}
fn info_help() -> String {
    String::from("Get full detailed list of real-time online players and active server metadata.")
}
fn start_bridge_help() -> String {
    String::from("Use in a channel to bridge it with Minecraft chat")
}

/// Check if I'm alive!
#[poise::command(slash_command, prefix_command, help_text_fn = ping_help)]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("UwU Helloo!").await?;
    Ok(())
}

/// Get list of online players in game right now
#[poise::command(
    slash_command,
    prefix_command,
    aliases("players", "now_playing", "online_players"),
    help_text_fn = info_help
)]
async fn info(ctx: Context<'_>) -> Result<(), Error> {
    let _ = ctx.defer().await;

    let query_address =
        env::var("MC_SERVER_QUERY_ADDRESS").unwrap_or_else(|_| "localhost:25565".to_string());

    // 1. Fetch exact player allocations via locked RCON connection
    let rcon_guard = ctx.data().rcon_client.lock().await;
    let rcon_response = match rcon_guard.send_command("list") {
        Ok(res) => res,
        Err(e) => format!("Error executing RCON list: {:?}", e),
    };
    drop(rcon_guard);

    let parsed_players: Vec<String> = if rcon_response.contains("online:") {
        if let Some((_, names_blob)) = rcon_response.split_once("online:") {
            names_blob
                .split(',')
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // 2. Extract layout statistics from TCP Server Status Ping
    let mut motd = "Minecraft Server Status".to_string();
    let mut latency_ms = 0.0;
    let mut favicon_b64: Option<String> = None;
    let mut total_players_online = 0;
    let mut max_players_limit = 20;

    if let Ok(status) = ctx
        .data()
        .mc_status_client
        .ping(&query_address, ServerEdition::Java)
        .await
    {
        let status = status;
        latency_ms = status.latency;
        if let ServerData::Java(java_data) = status.data {
            motd = java_data.description;
            favicon_b64 = java_data.favicon;
            total_players_online = java_data.players.online;
            max_players_limit = java_data.players.max;
        }
    }

    // 3. Assemble Embed Output Payload
    let mut embed_description = String::new();
    if parsed_players.is_empty() {
        if total_players_online > 0 {
            embed_description
                .push_str("⚠️ *Failed to safely map names via RCON, but players are active.*");
        } else {
            embed_description.push_str("*No players are currently online.*");
        }
    } else {
        embed_description.push_str("👥 **Current Online Players:**\n\n");
        for (index, player_name) in parsed_players.iter().enumerate() {
            embed_description.push_str(&format!("{}. `{}`\n", index + 1, player_name));
        }
    }

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("🎮 {motd}"))
        .description(embed_description)
        .color(0x9b59b6)
        .field(
            "Players Online",
            format!("`{total_players_online}/{max_players_limit}`"),
            true,
        )
        .field("Latency", format!("`{latency_ms:.1}ms`"), true);

    let mut reply = poise::CreateReply::default();

    // Decode and route favicon safely via internal binary attachments
    if let Some(base64_data) = favicon_b64 {
        let clean_b64 = base64_data.replace("data:image/png;base64,", "");
        if let Ok(image_bytes) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, clean_b64)
        {
            let attachment = serenity::CreateAttachment::bytes(image_bytes, "server_icon.png");
            reply = reply.attachment(attachment);
            embed = embed.thumbnail("attachment://server_icon.png");
        }
    }

    ctx.send(reply.embed(embed)).await?;
    Ok(())
}

/// Link Minecraft log parsing events directly into this channel
#[poise::command(
    slash_command,
    prefix_command,
    help_text_fn = start_bridge_help,
    check = "is_owner_or_admin"
)]
pub async fn start_bridge(ctx: Context<'_>) -> Result<(), Error> {
    let current_channel_id = ctx.channel_id();
    let shared_list = &ctx.data().target_channel_id_list;

    {
        let mut lock = shared_list.write().await;
        if let Some(ref mut channels) = *lock {
            if !channels.contains(&current_channel_id) {
                channels.push(current_channel_id);
            }
        } else {
            *lock = Some(vec![current_channel_id]);
        }
    }

    ctx.say(format!(
        "🟢 **Bridge Established!** Minecraft chat will now sync to <#{current_channel_id}>."
    ))
    .await?;
    Ok(())
}

/// Sever the active live-chat stream connection in this channel
#[poise::command(slash_command, prefix_command, check = "is_owner_or_admin")]
pub async fn stop_bridge(ctx: Context<'_>) -> Result<(), Error> {
    let current_channel_id = ctx.channel_id();
    let shared_list = &ctx.data().target_channel_id_list;

    let mut lock = shared_list.write().await;
    let was_bridged = if let Some(ref mut channels) = *lock {
        if let Some(index) = channels.iter().position(|&id| id == current_channel_id) {
            channels.remove(index);
            if channels.is_empty() {
                *lock = None;
            }
            true
        } else {
            false
        }
    } else {
        false
    };

    if was_bridged {
        ctx.send(
            poise::CreateReply::default().embed(
                serenity::CreateEmbed::new()
                    .title("🛑 Bridge Severed!")
                    .description(format!(
                        "The live-chat stream to <#{current_channel_id}> has been disconnected."
                    ))
                    .color(0xe74c3c),
            ),
        )
        .await?;
    } else {
        ctx.say(format!(
            "❌ This channel (<#{current_channel_id}>) isn't currently bound to an active bridge."
        ))
        .await?;
    }
    Ok(())
}

async fn is_owner_or_admin(ctx: Context<'_>) -> Result<bool, Error> {
    let user_id = ctx.author().id;
    if ctx.framework().options().owners.contains(&user_id) {
        return Ok(true);
    }

    if let Some(guild_id) = ctx.guild_id() {
        if let Some(member) = ctx.author_member().await {
            if let Some(guild) = guild_id.to_guild_cached(ctx.serenity_context()) {
                if guild
                    .member_permissions(&member)
                    .contains(serenity::Permissions::ADMINISTRATOR)
                {
                    return Ok(true);
                }
            }
        }
    }

    ctx.say("❌ **Access Denied:** This command is restricted to the Bot Owner and Server Administrators.").await?;
    Ok(false)
}

/// Show all available commands or get detailed help for a specific one
#[poise::command(slash_command, prefix_command)]
pub async fn help(ctx: Context<'_>, command_name: Option<String>) -> Result<(), Error> {
    if let Some(target) = command_name {
        if let Some(command) = ctx
            .framework()
            .options()
            .commands
            .iter()
            .find(|c| c.name == target)
        {
            let detailed_help = if let Some(help_fn) = command.help_text.clone() {
                help_fn.clone()
            } else {
                command
                    .help_text
                    .as_deref()
                    .unwrap_or("No detailed documentation available for this command.")
                    .to_string()
            };

            ctx.send(
                poise::CreateReply::default().embed(
                    serenity::CreateEmbed::new()
                        .title(format!("ℹ️ Detailed Help: /{}", command.name))
                        .description(detailed_help)
                        .color(0x3498db),
                ),
            )
            .await?;
            return Ok(());
        }
        ctx.say(format!("❌ Command `{target}` not found.")).await?;
        return Ok(());
    }

    let mut embed_fields = Vec::new();
    for command in &ctx.framework().options().commands {
        let description = command
            .description
            .as_deref()
            .unwrap_or("No description provided.");
        embed_fields.push((
            format!("`~{}`", command.name),
            description.to_string(),
            false,
        ));
    }

    let embed = serenity::CreateEmbed::new()
        .title("💥 Hello! こんにちは！~\n\n")
        .description("Here's a list of all the commands you can use:")
        .color(0x3498db)
        .fields(embed_fields)
        .footer(serenity::CreateEmbedFooter::new("Use ~command or \"hey reze, command\" to use any of these commands"))
        .thumbnail("https://media1.giphy.com/media/v1.Y2lkPTc5MGI3NjExN3hja2kyZ3NqdXFxZHlzMWowNXdxcWtpMzA3aW9hNGVuNngwcDZ4OCZlcD12MV9pbnRlcm5hbF9naWZfYnlfaWQmY3Q9Zw/IKFVtPf8jP6KJH16dB/giphy.gif");

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
