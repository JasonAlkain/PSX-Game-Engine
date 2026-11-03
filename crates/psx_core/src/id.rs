//! Persistent identifiers stored in project files.
//!
//! Entity and asset IDs are random (version 4) UUIDs. They are written to
//! scene files and asset metadata and never change once assigned, so moving
//! or renaming an entity or a source file does not break references. Runtime
//! handles (ECS entities, GPU buffers, physics bodies) are never serialised;
//! they are resolved from these IDs when a scene is loaded.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! persistent_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// A fresh random ID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Hyphenated lower-case, the same text as in files.
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

persistent_id!(
    /// Persistent identity of an entity in a scene.
    EntityId
);
persistent_id!(
    /// Persistent identity of an imported asset (stored in its `.meta.json` sidecar).
    AssetId
);
persistent_id!(
    /// Persistent identity of a scene document.
    SceneId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip_as_plain_strings() {
        let a = EntityId::new();
        let b = EntityId::new();
        assert_ne!(a, b);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, format!("\"{a}\""));
        assert_eq!(serde_json::from_str::<EntityId>(&json).unwrap(), a);
        assert_eq!(a.to_string().parse::<EntityId>().unwrap(), a);
        assert!("not-a-uuid".parse::<AssetId>().is_err());
    }
}
