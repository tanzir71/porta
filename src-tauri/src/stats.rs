use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStats {
    pub visitors: u64,
    pub requests: u64,
    pub bytes_served: u64,
}
