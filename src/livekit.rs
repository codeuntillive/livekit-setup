use livekit_api::access_token::{AccessToken, AccessTokenError, AudioGrants};
use std::env;

/// Creates a short-lived token that allows the browser to join the configured room.
pub fn create_token(identity: &str, room: &str) -> Result<String, AccessTokenError> {
    let api_key = env::var("LIVEKIT_API_KEY").expect("LIVEKIT_API_KEY is not set");
    let api_secret = env::var("LIVEKIT_API_SECRET").expect("LIVEKIT_API_SECRET is not set");

    AccessToken::with_api_key(&api_key, &api_secret)
        .with_identity(identity)
        .with_name(identity)
        .with_ttl(3600)
        .with_grants(AudioGrants {
            room_join: true,
            room: room.to_owned(),
            can_publish: true,
            can_subscribe: true,
            ..Default::default()
        })
        .to_jwt()
}
