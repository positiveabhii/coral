use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use coral_api::DEFAULT_WORKSPACE_ID;

use crate::bootstrap::AppError;
use crate::identity::parse_path_segment;

/// App-owned identity for one validated workspace name.
///
/// `coral-app` keeps workspace identity as this narrow type throughout app
/// state, managers, and layout code so those layers do not depend on transport
/// message shapes. Strings are normalized into `WorkspaceName` at persistence
/// and service edges before app logic runs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WorkspaceName(String);

impl WorkspaceName {
    /// Parse and validate a workspace name for app-internal use.
    pub(crate) fn parse(name: &str) -> Result<Self, AppError> {
        parse_path_segment("workspace", name).map(Self)
    }

    /// Borrow the normalized workspace name for filesystem and persistence
    /// boundaries that still operate on strings.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for WorkspaceName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WorkspaceName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

impl FromStr for WorkspaceName {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Default for WorkspaceName {
    fn default() -> Self {
        Self(DEFAULT_WORKSPACE_ID.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_WORKSPACE_ID, WorkspaceName};

    #[test]
    fn parses_default_workspace_name() {
        assert_eq!(WorkspaceName::default().as_str(), DEFAULT_WORKSPACE_ID);
    }
}
