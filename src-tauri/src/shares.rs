use serde::{Deserialize, Deserializer, Serialize};

use crate::stats::ShareStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareKind {
    Folder,
    Port,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareStatus {
    Stopped,
    Starting,
    Live,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub id: String,
    pub kind: ShareKind,
    pub name: String,
    pub path: Option<String>,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub status: ShareStatus,
    pub error: Option<String>,
    pub password_protected: bool,
    pub show_listing: bool,
    pub allow_uploads: bool,
    pub auto_start: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub stats: ShareStats,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShareInput {
    pub kind: ShareKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_listing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_uploads: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_now: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateShareInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub password: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_listing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_uploads: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_id: Option<Option<String>>,
}

fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    ShareChanged { share: Share },
    ShareRemoved { id: String },
    StatsUpdated { id: String, stats: ShareStats },
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::Value;

    use super::{AppEvent, CreateShareInput, Share, UpdateShareInput};
    use crate::settings::Settings;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct IpcContractFixture {
        share: Share,
        settings: Settings,
        create_share_input: CreateShareInput,
        update_share_input: UpdateShareInput,
        event: AppEvent,
    }

    #[test]
    fn deserializes_and_round_trips_typescript_contract_fixture() {
        let raw = include_str!("../tests/fixtures/ipc_contract.json");
        let raw_value: Value = serde_json::from_str(raw).expect("fixture should be valid JSON");
        let fixture: IpcContractFixture =
            serde_json::from_str(raw).expect("fixture should match the IPC contract");

        assert_eq!(fixture.update_share_input.password, Some(None));
        assert_eq!(
            serde_json::to_value(&fixture.share).expect("share should serialize"),
            raw_value["share"]
        );
        assert_eq!(
            serde_json::to_value(&fixture.settings).expect("settings should serialize"),
            raw_value["settings"]
        );
        assert_eq!(
            serde_json::to_value(&fixture.create_share_input)
                .expect("create input should serialize"),
            raw_value["createShareInput"]
        );
        assert_eq!(
            serde_json::to_value(&fixture.update_share_input)
                .expect("update input should serialize"),
            raw_value["updateShareInput"]
        );
        assert_eq!(
            serde_json::to_value(&fixture.event).expect("event should serialize"),
            raw_value["event"]
        );
    }
}
