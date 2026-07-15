use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub launch_at_login: bool,
    pub auto_start_shares: bool,
    pub show_dock_icon: bool,
    pub notify_on_first_visitor: bool,
    pub copy_url_on_start: bool,
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            auto_start_shares: true,
            show_dock_icon: true,
            notify_on_first_visitor: true,
            copy_url_on_start: true,
            theme: Theme::System,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_at_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_start_shares: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_dock_icon: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_on_first_visitor: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_url_on_start: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
}
