use std::{collections::HashSet, sync::Arc, time::Duration};

use rust_mc_status::McClient;

use ::serenity::{
    all::prelude::Mentionable,
    builder::{CreateEmbed, CreateEmbedFooter},
    model::id::ChannelId,
};
use poise::serenity_prelude as serenity;
use tokio::sync::{
    RwLock,
    mpsc::{self, Receiver, Sender},
};

use crate::discord_bot::{FromDiscordEvent, FromMinecraftEvent};

type Context<'a> = poise::Context<'a, Data, Error>;

#[derive(Clone)]
struct Data {
    dc_event_tx: mpsc::Sender<FromDiscordEvent>,
    mc_status_client: McClient,
    target_channel_id: std::sync::Arc<tokio::sync::RwLock<Option<ChannelId>>>,
}

//hardcoding my own id
const OWNER_ID: u64 = 1314616785156444175;

// Define  error type (using a standard Boxed dynamic error)
type Error = Box<dyn std::error::Error + Send + Sync>;

pub async fn start_dc_bot(
    mut mc_event_rx: Receiver<FromMinecraftEvent>,
    dc_event_tx: Sender<FromDiscordEvent>,
) {
    let token = std::env::var("DISCORD_TOKEN").expect("missing DISCORD_TOKEN");
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let bridge_channel_id = Arc::new(RwLock::new(None));

    let bridge_channel_id_clone = Arc::clone(&bridge_channel_id);
    let client = McClient::new()
        .with_timeout(Duration::from_secs(5))
        .with_max_parallel(10);

    let mut owners = HashSet::new();
    owners.insert(serenity::UserId::new(OWNER_ID));

    let framework: poise::Framework<Data, Error> = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            event_handler: |ctx, event, _, data| Box::pin(event_handler(ctx, event, data)),
            commands: vec![ping(), start_bridge(), online_players(), help()],
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
        .setup(|_, _ready, _framework| {
            Box::pin(async move {
                // Return an instance of  Data struct
                Ok(Data {
                    dc_event_tx: dc_event_tx,
                    mc_status_client: client,
                    target_channel_id: bridge_channel_id.clone(),
                })
            })
        })
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;

    let mut client = client.unwrap();

    let cache_http = Arc::clone(&client.http);

    tokio::spawn(async move {
        while let Some(event) = mc_event_rx.recv().await {
            let formatted_message = format!("**{}**: {}", event.username, event.content);

            let current_target = {
                let lock = bridge_channel_id_clone.read().await;
                *lock // Copies out the underlying Option<ChannelId>
            };

            match current_target {
                Some(target_channel) => {
                    println!(
                        "sending msg to discord channel [{}]: {}",
                        target_channel, formatted_message
                    );
                    if let Err(why) = target_channel.say(&cache_http, formatted_message).await {
                        println!("failed to send message to discord {why:?}");
                    }
                }
                None => {
                    println!("Bridge not active yet. Ignored: {}", formatted_message);
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
                .color(0x9b59b6) // Reze Purple Aesthetic
                .thumbnail(
                    "https://i.pinimg.com/originals/5d/15/4b/5d154b68de57a87600fe9b98d692802c.gif",
                ) // Reze portrait
                .footer(serenity::CreateEmbedFooter::new(format!(
                    "Member Count: #{}",
                    new_member
                        .guild_id
                        .to_guild_cached(ctx)
                        .map(|g| g.member_count)
                        .unwrap_or(0)
                )));

            // 3. Send the message to the channel
            let message = serenity::CreateMessage::new().embed(welcome_embed);
            let _ = system_channel.send_message(&ctx.http, message).await;
        }
        serenity::FullEvent::Message { new_message } => {
            let target_channel = {
                let lock = data.target_channel_id.read().await;
                *lock
            };
            if let Some(target_channel) = target_channel {
                if new_message.channel_id == target_channel
                    && new_message.author.id != ctx.cache.clone().current_user().id
                {
                    if let Err(why) = data
                        .dc_event_tx
                        .send(FromDiscordEvent {
                            username: new_message.author.name.clone(),
                            content: new_message.content.clone(),
                        })
                        .await
                    {
                        println!("failed to send FromDiscordEvent: {why:?}")
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

///Check if I'm alive!
#[poise::command(slash_command, prefix_command, help_text_fn=ping_help)]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let _ = ctx.say("UwU Helloo!").await;
    Ok(())
}

///Get a list of online players in game right now
#[poise::command(
    slash_command,
    prefix_command,
    aliases("players, now_playing"),
    help_text_fn = "online_players_help"
)]
async fn online_players(ctx: Context<'_>) -> Result<(), Error> {
    let _ = ctx.defer().await;

    match ctx
        .data()
        .mc_status_client
        .ping_java("localhost:25565")
        .await
    {
        Ok(status) => {
            if let rust_mc_status::ServerData::Java(java_data) = status.data {
                let server_title = format!("🎮 {}", java_data.description);
                let mut description = String::new();

                if let Some(player_list) = &java_data.players.sample {
                    if player_list.is_empty() {
                        description.push_str("*No players are currently online.*");
                    } else {
                        description.push_str("👥 **Current Online Players:**\n\n");
                        for (index, player) in player_list.iter().enumerate() {
                            description.push_str(&format!("{}. `{}`\n", index + 1, player.name));
                        }
                    }
                } else {
                    description.push_str("*No player sample names returned.*");
                }

                ctx.send(
                    poise::CreateReply::default().embed(
                        serenity::CreateEmbed::new()
                            .title(server_title)
                            .description(description)
                            .color(0x55FF55)
                            .field(
                                "Players Online",
                                format!("`{}/{}`", java_data.players.online, java_data.players.max),
                                true,
                            )
                            .field(
                                "Version",
                                format!(
                                    "`{} (Proto {})`",
                                    java_data.version.name, java_data.version.protocol
                                ),
                                true,
                            )
                            .field("Latency", format!("`{:.1}ms`", status.latency), true),
                    ),
                )
                .await?;
            } else {
                ctx.say("❌ Error: Targeted server returned Bedrock protocol instead of Java.")
                    .await?;
            }
        }
        Err(why) => {
            ctx.say(format!("❌ Failed to reach Minecraft Server: {:?}", why))
                .await?;
        }
    }
    Ok(())
}

fn start_bridge_help() -> String {
    String::from(
        "Use in channel where you want to start chat bridge between Discord and Minecraft\\ This command is a Owner/Admin only command",
    )
}

///start Minecraft to Discord chat bridge
#[poise::command(
    slash_command,
    prefix_command,
    help_text_fn = "start_bridge_help",
    check = "is_owner_or_admin"
)]
pub async fn start_bridge(ctx: Context<'_>) -> Result<(), Error> {
    let current_channel_id = ctx.channel_id();
    let shared_channel = &ctx.data().target_channel_id;
    {
        let mut lock = shared_channel.write().await;
        *lock = Some(current_channel_id);
    }
    ctx.say(format!(
        "🟢 **Bridge Established!** Minecraft chat will now sync to <#{}>.",
        current_channel_id
    ))
    .await?;
    Ok(())
}

//bullshit code
// async fn is_owner_or_admin(ctx: Context<'_>) -> Result<bool, Error> {
//     let user_id = ctx.author().id;
//
//     let owners = &ctx.framework().options().owners;
//     if owners.contains(&user_id) {
//         return Ok(true);
//     }
//
//     if let Some(guild_id) = ctx.guild_id() {
//         if let Some(member) = ctx
//             .serenity_context()
//             .http
//             .get_member(guild_id, user_id)
//             .await
//             .ok()
//         {
//             if let Some(guild) = ctx.guild() {
//                 if let Some(guild_channel) = ctx.guild_channel().await {
//                     if let Ok(member) = guild.member(ctx.http(), user_id).await {
//                         let permissions =
//                             guild.user_permissions_in(&guild_channel, member.as_ref());
//                         {
//                             if permissions.contains(serenity::Permissions::ADMINISTRATOR) {
//                                 return Ok(true);
//                             }
//                         }
//                     }
//                 }
//             }
//         }
//     }
//
//     ctx.say("❌ **Access Denied:** This command is restricted to the Bot Owner and Server Administrators.").await?;
//     Ok(false)
// }

async fn is_owner_or_admin(ctx: Context<'_>) -> Result<bool, Error> {
    let user_id = ctx.author().id;

    let owners = &ctx.framework().options().owners;
    if owners.contains(&user_id) {
        return Ok(true);
    }

    if let Some(guild_id) = ctx.guild_id() {
        if let Some(member) = ctx.author_member().await {
            if let Some(guild) = guild_id.to_guild_cached(ctx.serenity_context()) {
                let permissions = guild.member_permissions(&member);

                if permissions.contains(serenity::Permissions::ADMINISTRATOR) {
                    return Ok(true);
                }
            }
        }
    }

    ctx.say("❌ **Access Denied:** This command is restricted to the Bot Owner and Server Administrators.").await?;
    Ok(false)
}

///Show all available commands or get detailed help for a specific one
#[poise::command(slash_command, prefix_command)]
pub async fn help(ctx: Context<'_>, command_name: Option<String>) -> Result<(), Error> {
    if let Some(target) = command_name {
        // Search through registered commands for a match
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

        ctx.say(format!("❌ Command `{}` not found.", target))
            .await?;
        return Ok(());
    }

    let footer = CreateEmbedFooter::new(
        "Use ~command or \"hey reze, command\" to use any of these commands",
    );

    let mut embed_fields = Vec::new();

    for command in &ctx.framework().options().commands {
        let description = command
            .description
            .as_deref()
            .unwrap_or("No description provided.");

        let command_name = format!("`~{}`", command.name);

        embed_fields.push((command_name, description.to_string(), false));
    }
    let embed = CreateEmbed::new()
        .title("💥 Hello! こんにちは！Hola! Bonjour!~\n\n")
        .description("Here's a list of all the commands you can use:")
        .color(0x3498db)
        .fields(embed_fields)
        .footer(footer)
        .thumbnail("https://media1.giphy.com/media/v1.Y2lkPTc5MGI3NjExN3hja2kyZ3NqdXFxZHlzMWowNXdxcWtpMzA3aW9hNGVuNngwcDZ4OCZlcD12MV9pbnRlcm5hbF9naWZfYnlfaWQmY3Q9Zw/IKFVtPf8jP6KJH16dB/giphy.gif");

    let _ = ctx.send(poise::CreateReply::default().embed(embed)).await;

    Ok(())
}

fn ping_help() -> String {
    String::from("Use this to check if I'm alive!")
}
fn online_players_help() -> String {
    String::from("Get list of online players in game right now")
}
