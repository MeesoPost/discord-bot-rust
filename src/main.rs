mod handler;
mod commands;
mod utils;

use std::env;
use serenity::prelude::*;
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
    
    // Get creator channel ID
    let creator_channel_id = ChannelId(
        env::var("CREATOR_CHANNEL_ID")
            .expect("Creator channel ID niet gevonden")
            .parse()
            .expect("Invalid channel ID")
    );

    // Set intents
    let intents = GatewayIntents::GUILDS 
        | GatewayIntents::GUILD_VOICE_STATES;

    // Create client
    let mut client = Client::builder(&token, intents)
        .event_handler(Handler::new(creator_channel_id))
        .await
        .expect("Error bij maken client");

    // Start bot
    if let Err(why) = client.start().await {
        println!("Client error: {:?}", why);
    }
}