//! Message data the parser produces and the app stores and serves as-is.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    #[serde(default)]
    pub content_id: Option<String>,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeInfo {
    pub methods: Vec<UnsubscribeMethod>,
    pub list_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UnsubscribeMethod {
    OneClick,
    Mailto,
    Web,
}
