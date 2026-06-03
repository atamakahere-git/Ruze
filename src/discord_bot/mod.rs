pub mod dc_bot;

pub struct FromMinecraftEvent {
    pub username: String,
    pub content: String,
}

#[derive(Debug)]
pub enum MinecraftEvent {
    Chat {
        username: String,
        message: String,
    },
    Death {
        system_message: String,
    },
    PlayerJoinLeave {
        system_message: String,
        is_join: bool,
    },
    Advancement {
        system_message: String,
    },
}

#[derive(Clone)]
pub struct FromDiscordEvent {
    pub username: String,
    pub content: String,
}
