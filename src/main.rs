mod handler;
mod commands;
mod utils;

use std::env;
use serenity::prelude::*;
use serenity::model::prelude::ChannelId;
use dotenv::dotenv;
use handler::Handler;

#[tokio::main]
async fn main() {
    // Initialize logger
    tracing_subscriber::fmt::init();

    // Load .env file
    dotenv().ok();
    
    // Get bot token
    let token = env::var("DISCORD_TOKEN")
        .expect("Token niet gevonden");
    
    // Get channel IDs
    let creator_channel_id = ChannelId(
        env::var("CREATOR_CHANNEL_ID")
            .expect("Creator channel ID niet gevonden")
            .parse()
            .expect("Invalid channel ID")
    );

    let waiting_room_id = ChannelId(
        env::var("WAITING_ROOM_ID")
            .expect("Waiting room ID niet gevonden")
            .parse()
            .expect("Invalid channel ID")
    );

    // Set intents
    let intents = GatewayIntents::GUILDS 
        | GatewayIntents::GUILD_VOICE_STATES;

    // Create client
    let mut client = Client::builder(&token, intents)
        .event_handler(Handler::new(creator_channel_id, waiting_room_id))
        .await
        .expect("Error bij maken client");

    // Start bot
    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}