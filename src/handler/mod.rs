// src/handler/mod.rs

use std::{collections::HashMap, sync::Arc, time::Duration};
use serenity::{
    async_trait,
    model::{
        gateway::Ready,
        voice::VoiceState,
        id::{ChannelId, UserId},
        channel::PermissionOverwrite,
        permissions::{Permissions, PermissionOverwriteType},
    },
    prelude::*,
};
use tokio::{sync::RwLock, time::sleep};
use tracing::{error, info, warn};

// Structuur voor de informatie over een tijdelijk kanaal
#[derive(Debug)]
pub struct ChannelInfo {
    owner_id: UserId,
    delete_task: Option<tokio::task::JoinHandle<()>>,
}

// De hoofdstructuur voor de bot handler
pub struct Handler {
    temp_channels: Arc<RwLock<HashMap<ChannelId, ChannelInfo>>>,
    creator_channel_id: ChannelId,
}

impl Handler {
    pub fn new(creator_channel_id: ChannelId) -> Self {
        Self {
            temp_channels: Arc::new(RwLock::new(HashMap::new())),
            creator_channel_id,
        }
    }

    // Functie om een tijdelijk kanaal aan te maken
    async fn create_temp_channel(
        &self,
        ctx: &Context,
        guild_id: GuildId,
        member: &Member,
        parent_id: Option<ChannelId>,
    ) -> Result<Channel, SerenityError> {
        let channel_name = format!("{}'s Channel", member.display_name());
        let bot_id = ctx.cache.current_user_id();

        let channel = guild_id.create_channel(&ctx.http, |c| {
            c.name(&channel_name)
                .kind(ChannelType::Voice)
                .permissions(vec![
                    // Standaard deny voor @everyone
                    PermissionOverwrite {
                        kind: PermissionOverwriteType::Role(guild_id.0.into()),
                        allow: Permissions::empty(),
                        deny: Permissions::CONNECT,
                    },
                    // Allow voor kanaal eigenaar
                    PermissionOverwrite {
                        kind: PermissionOverwriteType::Member(member.user.id),
                        allow: Permissions::CONNECT
                            | Permissions::MOVE_MEMBERS
                            | Permissions::MANAGE_CHANNELS
                            | Permissions::MUTE_MEMBERS
                            | Permissions::DEAFEN_MEMBERS,
                        deny: Permissions::empty(),
                    },
                    // Allow voor bot
                    PermissionOverwrite {
                        kind: PermissionOverwriteType::Member(bot_id),
                        allow: Permissions::CONNECT
                            | Permissions::MOVE_MEMBERS
                            | Permissions::MANAGE_CHANNELS,
                        deny: Permissions::empty(),
                    },
                ]);

            // Als er een parent category is, zet het kanaal in die category
            if let Some(parent) = parent_id {
                c.parent_id(parent);
            }
            c
        })
        .await?;

        info!("✓ Kanaal aangemaakt: {}", channel_name);
        Ok(channel)
    }

    // Functie om een verwijdertaak voor een kanaal aan te maken
    async fn schedule_channel_deletion(
        &self,
        ctx: Context,
        channel_id: ChannelId,
        channel_name: String,
    ) -> tokio::task::JoinHandle<()> {
        let temp_channels = Arc::clone(&self.temp_channels);

        tokio::spawn(async move {
            sleep(Duration::from_secs(5)).await;

            match channel_id.delete(&ctx.http).await {
                Ok(_) => {
                    info!("✓ Kanaal {} verwijderd", channel_name);
                    let mut channels = temp_channels.write().await;
                    channels.remove(&channel_id);
                }
                Err(e) => error!("Fout bij verwijderen kanaal {}: {:?}", channel_name, e),
            }
        })
    }
}

#[async_trait]
impl EventHandler for Handler {
    // Event handler voor als de bot opstart
    async fn ready(&self, _: Context, ready: Ready) {
        info!("Bot is online als {}!", ready.user.name);
        info!("Watching creator channel ID: {}", self.creator_channel_id);
    }

    // Event handler voor voice state updates
    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        // Gebruiker joint creator kanaal
        if let Some(channel_id) = new.channel_id {
            if channel_id == self.creator_channel_id {
                let guild_id = match new.guild_id {
                    Some(id) => id,
                    None => return,
                };

                // Check permissions
                let guild = match guild_id.to_guild_cached(&ctx.cache) {
                    Some(g) => g,
                    None => {
                        warn!("Guild niet gevonden in cache");
                        return;
                    }
                };

                let bot_id = ctx.cache.current_user_id();
                let bot_member = match guild.member(&ctx.http, bot_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        error!("Kon bot member niet ophalen: {:?}", e);
                        return;
                    }
                };

                if !bot_member
                    .permissions(&ctx.cache)
                    .map_or(false, |p| p.manage_channels())
                {
                    error!("Bot mist de benodigde permissies!");
                    return;
                }

                // Maak nieuw kanaal
                let member = match new.member {
                    Some(ref m) => m,
                    None => return,
                };

                let parent_id = new
                    .channel_id
                    .to_channel_cached(&ctx.cache)
                    .and_then(|c| c.parent_id);

                match self
                    .create_temp_channel(&ctx, guild_id, member, parent_id)
                    .await
                {
                    Ok(channel) => {
                        // Voeg kanaal toe aan temp_channels
                        {
                            let mut temp_channels = self.temp_channels.write().await;
                            temp_channels.insert(
                                channel.id,
                                ChannelInfo {
                                    owner_id: member.user.id,
                                    delete_task: None,
                                },
                            );
                        }

                        // Verplaats gebruiker
                        if let Err(e) = member.move_to_voice_channel(&ctx.http, channel.id).await {
                            error!("Error bij verplaatsen gebruiker: {:?}", e);
                        } else {
                            info!("✓ Gebruiker verplaatst naar nieuw kanaal");
                        }
                    }
                    Err(e) => error!("Fout bij aanmaken kanaal: {:?}", e),
                }
            }
        }

        // Gebruiker verlaat een kanaal
        if let Some(old_state) = old {
            if let Some(old_channel_id) = old_state.channel_id {
                let mut temp_channels = self.temp_channels.write().await;

                if let Some(channel_info) = temp_channels.get_mut(&old_channel_id) {
                    let guild = match old_state
                        .guild_id
                        .and_then(|id| id.to_guild_cached(&ctx.cache))
                    {
                        Some(g) => g,
                        None => return,
                    };

                    let channel = match guild.channels.get(&old_channel_id) {
                        Some(c) => c,
                        None => return,
                    };

                    // Check of het kanaal leeg is
                    if channel.members(&ctx.cache).count() == 0 {
                        info!(
                            "Kanaal {} is leeg, wordt over 5 seconden verwijderd",
                            channel.name()
                        );

                        // Cancel bestaande delete task als die er is
                        if let Some(task) = channel_info.delete_task.take() {
                            task.abort();
                        }

                        // Start nieuwe delete task
                        let delete_task = self
                            .schedule_channel_deletion(
                                ctx.clone(),
                                old_channel_id,
                                channel.name().to_string(),
                            )
                            .await;

                        channel_info.delete_task = Some(delete_task);
                    }
                }
            }
        }

        // Gebruiker joint een bestaand tijdelijk kanaal
        if let Some(new_channel_id) = new.channel_id {
            let mut temp_channels = self.temp_channels.write().await;

            if let Some(channel_info) = temp_channels.get_mut(&new_channel_id) {
                // Cancel verwijdertaak als die bestaat
                if let Some(task) = channel_info.delete_task.take() {
                    task.abort();
                    info!("Verwijdering van kanaal geannuleerd omdat er iemand gejoind is");
                }
            }
        }
    }
}