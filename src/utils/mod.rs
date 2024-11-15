use serenity::{model::prelude::*, prelude::*};

pub async fn check_permissions(ctx: &Context, guild_id: GuildId) -> bool {
    let guild = match guild_id.to_guild_cached(&ctx.cache) {
        Some(guild) => guild,
        None => return false,
    };

    let bot_user_id = ctx.cache.current_user_id();
    let bot_member = match guild.member(&ctx.http, bot_user_id).await {
        Ok(member) => member,
        Err(_) => return false,
    };

    bot_member.permissions(&ctx.cache).map_or(false, |p| p.manage_channels())
}