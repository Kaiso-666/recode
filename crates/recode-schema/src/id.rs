use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_id {
    ($name:ident, $prefix:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Generate a new time-sortable identifier with the type prefix.
            pub fn new() -> Self {
                Self(format!("{}{}", $prefix, uuid::Uuid::now_v7().simple()))
            }

            /// Adopt an identifier that already carries the correct prefix.
            pub fn from_existing(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn has_prefix(&self) -> bool {
                self.0.starts_with($prefix)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.to_owned()))
            }
        }
    };
}

define_id!(SessionId, "ses_", "A durable conversation identifier.");
define_id!(MessageId, "msg_", "A message within a session.");
define_id!(PartId, "prt_", "A part (block) within a message.");
define_id!(EventId, "evt_", "A durable session event.");
define_id!(ProjectId, "prj_", "A project (repository) identifier.");
define_id!(ToolCallId, "cal_", "A tool invocation identifier.");

impl ToolCallId {
    /// Providers supply their own opaque call IDs; keep them verbatim.
    pub fn from_provider(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl MessageId {
    /// The assistant message generated for a step derives from the event that
    /// started it, mirroring opencode's `msg_`/`evt_` relationship.
    pub fn from_event(event: &EventId) -> Self {
        Self(event.0.replacen("evt_", "msg_", 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert!(a.as_str().starts_with("ses_"));
        assert_ne!(a, b);
    }

    #[test]
    fn ids_round_trip_through_json() {
        let id = MessageId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id));
        let back: MessageId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
