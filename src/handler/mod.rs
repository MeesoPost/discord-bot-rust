use serenity::{
    async_trait,
    model::{
        gateway::Ready,
        voice::VoiceState,
        id::{ChannelId, GuildId, UserId},
        channel::{Channel, ChannelType, PermissionOverwrite},
        guild::Member,
        permissions::Permissions,
        prelude::PermissionOverwriteType,
    },
    prelude::*,
};
use tokio::{sync::RwLock, time::sleep};
use tracing::{error, info, warn};
use std::{collections::HashMap, sync::Arc, time::Duration};

#[derive(Debug)]
pub struct ChannelInfo {
    owner_id: UserId,
    delete_task: Option<tokio::task::JoinHandle<()>>,
}

pub struct Handler {
    temp_channels: Arc<RwLock<HashMap<ChannelId, ChannelInfo>>>,
    creator_channel_id: ChannelId,
    waiting_room_id: ChannelId,
}

impl Handler {
    pub fn new(creator_channel_id: ChannelId, waiting_room_id: ChannelId) -> Self {
        Self {
            temp_channels: Arc::new(RwLock::new(HashMap::new())),
            creator_channel_id,
            waiting_room_id,
        }
    }

    async fn user_has_channel(&self, user_id: UserId) -> bool {
        let temp_channels = self.temp_channels.read().await;
        temp_channels.values().any(|info| info.owner_id == user_id)
    }

    async fn get_user_channel(&self, user_id: UserId) -> Option<ChannelId> {
        let temp_channels = self.temp_channels.read().await;
        temp_channels
            .iter()
            .find(|(_, info)| info.owner_id == user_id)
            .map(|(channel_id, _)| *channel_id)
    }

    async fn handle_creator_channel_join(
        &self,
        ctx: &Context,
        guild_id: GuildId,
        member: &Member,
        parent_id: Option<ChannelId>,
    ) -> Result<(), SerenityError> {
        // First, remove existing channel if it exists
        if self.user_has_channel(member.user.id).await {
            if let Some(existing_channel) = self.get_user_channel(member.user.id).await {
                // Delete the existing channel
                if let Err(e) = existing_channel.delete(&ctx.http).await {
                    error!("Error deleting existing channel: {:?}", e);
                } else {
                    info!("Successfully deleted existing channel");
                    // Remove from tracking
                    let mut temp_channels = self.temp_channels.write().await;
                    temp_channels.remove(&existing_channel);
                }
            }
        }

        // Create a new channel
        match self.create_temp_channel(ctx, guild_id, member, parent_id).await {
            Ok(Channel::Guild(guild_channel)) => {
                {
                    let mut temp_channels = self.temp_channels.write().await;
                    temp_channels.insert(
                        guild_channel.id,
                        ChannelInfo {
                            owner_id: member.user.id,
                            delete_task: None,
                        },
                    );
                }

                if let Err(e) = member.move_to_voice_channel(&ctx.http, guild_channel.id).await {
                    error!("Error moving user: {:?}", e);
                } else {
                    info!("✓ User moved to new channel");
                }
            }
            _ => error!("Unexpected channel type created"),
        }
        Ok(())
    }

    async fn create_temp_channel(
        &self,
        ctx: &Context,
        guild_id: GuildId,
        member: &Member,
        parent_id: Option<ChannelId>,
    ) -> Result<Channel, SerenityError> {
        let channel_name = if let Some(guild) = guild_id.to_guild_cached(&ctx.cache) {
            if let Some(member_info) = guild.member(&ctx.http, member.user.id).await.ok() {
                member_info.display_name().to_string()
            } else {
                member.user.name.clone()
            }
        } else {
            member.user.name.clone()
        };
        let bot_id = ctx.cache.current_user_id();
        let waiting_room_id = self.waiting_room_id;

        let guild_channel = guild_id.create_channel(&ctx.http, |c| {
            let mut channel = c.name(&channel_name)
                .kind(ChannelType::Voice)
                .permissions(vec![
                    PermissionOverwrite {
                        kind: PermissionOverwriteType::Role(guild_id.0.into()),
                        allow: Permissions::empty(),
                        deny: Permissions::CONNECT | Permissions::MOVE_MEMBERS,
                    },
                    PermissionOverwrite {
                        kind: PermissionOverwriteType::Member(member.user.id),
                        allow: Permissions::CONNECT 
                            | Permissions::MANAGE_CHANNELS
                            | Permissions::MUTE_MEMBERS
                            | Permissions::DEAFEN_MEMBERS,
                        deny: Permissions::empty(),
                    },
                    PermissionOverwrite {
                        kind: PermissionOverwriteType::Member(bot_id),
                        allow: Permissions::CONNECT
                            | Permissions::MOVE_MEMBERS
                            | Permissions::MANAGE_CHANNELS,
                        deny: Permissions::empty(),
                    },
                ]);

            if let Some(parent) = parent_id {
                channel = channel.category(parent);
            }
            channel
        })
        .await?;

        waiting_room_id.create_permission(
            &ctx.http,
            &PermissionOverwrite {
                kind: PermissionOverwriteType::Member(member.user.id),
                allow: Permissions::MOVE_MEMBERS,
                deny: Permissions::empty(),
            },
        ).await?;

        info!("✓ Kanaal aangemaakt: {} met beperkte move permissions", channel_name);
        Ok(Channel::Guild(guild_channel))
    }

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
    async fn ready(&self, _: Context, ready: Ready) {
        info!("Bot is online als {}!", ready.user.name);
        info!("Watching creator channel ID: {}", self.creator_channel_id);
    }

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        if let Some(channel_id) = new.channel_id {
            if channel_id == self.creator_channel_id {
                let guild_id = match new.guild_id {
                    Some(id) => id,
                    None => return,
                };

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

                let member = match new.member {
                    Some(ref m) => m,
                    None => return,
                };

                let channel = new.channel_id
                    .expect("Channel ID should exist")
                    .to_channel_cached(&ctx.cache);

                let parent_id = channel.and_then(|c| match c {
                    Channel::Guild(gc) => gc.parent_id,
                    _ => None,
                });

                if let Err(e) = self.handle_creator_channel_join(&ctx, guild_id, member, parent_id).await {
                    error!("Error handling creator channel join: {:?}", e);
                }
            }
        }

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

                    match guild.channels.get(&old_channel_id) {
                        Some(channel) => {
                            match channel {
                                Channel::Guild(gc) => {
                                    match gc.members(&ctx).await {
                                        Ok(members) => {
                                            if members.is_empty() {
                                                info!(
                                                    "Kanaal {} is leeg, wordt over 5 seconden verwijderd",
                                                    gc.name
                                                );

                                                if let Some(task) = channel_info.delete_task.take() {
                                                    task.abort();
                                                }

                                                let delete_task = self
                                                    .schedule_channel_deletion(
                                                        ctx.clone(),
                                                        old_channel_id,
                                                        gc.name.clone(),
                                                    )
                                                    .await;

                                                channel_info.delete_task = Some(delete_task);
                                            }
                                        },
                                        Err(e) => error!("Fout bij ophalen kanaal members: {:?}", e),
                                    }
                                },
                                _ => warn!("Channel is not a guild channel"),
                            }
                        }
                        None => return,
                    };
                }
            }
        }

        if let Some(new_channel_id) = new.channel_id {
            let mut temp_channels = self.temp_channels.write().await;

            if let Some(channel_info) = temp_channels.get_mut(&new_channel_id) {
                if let Some(task) = channel_info.delete_task.take() {
                    task.abort();
                    info!("Verwijdering van kanaal geannuleerd omdat er iemand gejoind is");
                }
            }
        }
    }
}