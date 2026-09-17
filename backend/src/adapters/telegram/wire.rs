use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
pub(super) struct ApiResponse<T> {
    pub(super) ok: bool,
    pub(super) result: Option<T>,
    pub(super) error_code: Option<i64>,
    #[allow(dead_code)]
    pub(super) description: Option<String>,
    pub(super) parameters: Option<ResponseParameters>,
}

#[derive(Deserialize)]
pub(super) struct ResponseParameters {
    pub(super) retry_after: Option<i64>,
    #[allow(dead_code)]
    pub(super) migrate_to_chat_id: Option<Value>,
}

#[derive(Deserialize)]
pub(crate) struct RawUpdate {
    pub(super) update_id: Value,
    pub(super) message: Option<Value>,
    pub(super) edited_message: Option<Value>,
    #[serde(flatten)]
    pub(super) _other: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
pub(super) struct Message {
    pub(super) message_id: Option<Value>,
    pub(super) message_thread_id: Option<Value>,
    pub(super) from: Option<User>,
    pub(super) sender_chat: Option<Value>,
    pub(super) chat: Option<Chat>,
    pub(super) date: Option<Value>,
    pub(super) text: Option<Value>,
    #[serde(rename = "entities")]
    pub(super) _entities: Option<Value>,
    pub(super) is_topic_message: Option<Value>,
    pub(super) business_connection_id: Option<Value>,
    pub(super) sender_business_bot: Option<Value>,
    pub(super) direct_messages_topic: Option<Value>,
    #[serde(flatten)]
    pub(super) other: BTreeMap<String, Value>,
}

impl Message {
    pub(super) fn has_media_or_nontext(&self) -> bool {
        const MARKERS: &[&str] = &[
            "animation",
            "audio",
            "caption",
            "caption_entities",
            "contact",
            "dice",
            "document",
            "game",
            "invoice",
            "location",
            "paid_media",
            "photo",
            "poll",
            "sticker",
            "story",
            "successful_payment",
            "venue",
            "video",
            "video_note",
            "voice",
        ];
        MARKERS.iter().any(|key| self.other.contains_key(*key))
    }

    pub(super) fn has_service_or_rich_marker(&self) -> bool {
        const MARKERS: &[&str] = &[
            "boost_added",
            "channel_chat_created",
            "chat_background_set",
            "chat_shared",
            "connected_website",
            "delete_chat_photo",
            "direct_message_price_changed",
            "effect_id",
            "forum_topic_closed",
            "forum_topic_created",
            "forum_topic_edited",
            "forum_topic_reopened",
            "general_forum_topic_hidden",
            "general_forum_topic_unhidden",
            "giveaway",
            "giveaway_completed",
            "giveaway_created",
            "giveaway_winners",
            "group_chat_created",
            "is_from_offline",
            "is_paid_post",
            "left_chat_member",
            "message_auto_delete_timer_changed",
            "migrate_from_chat_id",
            "migrate_to_chat_id",
            "new_chat_members",
            "new_chat_photo",
            "new_chat_title",
            "passport_data",
            "paid_star_count",
            "pinned_message",
            "proximity_alert_triggered",
            "refunded_payment",
            "supergroup_chat_created",
            "suggested_post_approval_failed",
            "suggested_post_approved",
            "suggested_post_declined",
            "suggested_post_info",
            "suggested_post_paid",
            "suggested_post_refunded",
            "users_shared",
            "via_bot",
            "video_chat_ended",
            "video_chat_participants_invited",
            "video_chat_scheduled",
            "video_chat_started",
            "web_app_data",
            "write_access_allowed",
        ];
        self.business_connection_id.is_some()
            || self.sender_business_bot.is_some()
            || self.direct_messages_topic.is_some()
            || MARKERS.iter().any(|key| self.other.contains_key(*key))
    }
}

#[derive(Deserialize)]
pub(super) struct User {
    pub(super) id: Option<Value>,
    pub(super) is_bot: Option<Value>,
    #[allow(dead_code)]
    pub(super) username: Option<Value>,
    pub(super) has_topics_enabled: Option<Value>,
}

#[derive(Deserialize)]
pub(super) struct Chat {
    pub(super) id: Option<Value>,
    #[serde(rename = "type")]
    pub(super) kind: Option<Value>,
}

#[derive(Deserialize)]
pub(super) struct WebhookInfo {
    pub(super) url: Option<Value>,
}

#[derive(Serialize)]
pub(super) struct GetUpdatesRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) offset: Option<i64>,
    pub(super) limit: u8,
    pub(super) timeout: u8,
    pub(super) allowed_updates: [&'static str; 1],
}

#[derive(Serialize)]
pub(super) struct SendMessageRequest<'a> {
    pub(super) chat_id: i64,
    pub(super) text: &'a str,
}
