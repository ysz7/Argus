use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Error;

macro_rules! opaque_id {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        ///
        /// Identifiers are opaque, non-empty strings. Consumers must not parse
        /// them or rely on their format.
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Creates an identifier, rejecting empty strings.
            pub fn new(id: impl Into<String>) -> crate::Result<Self> {
                let id = id.into();
                if id.is_empty() {
                    return Err(Error::EmptyId { kind: $kind });
                }
                Ok(Self(id))
            }

            /// The identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = Error;

            fn try_from(id: String) -> crate::Result<Self> {
                Self::new(id)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

opaque_id!(
    /// Identifier of an [`Observation`](crate::Observation).
    ObservationId,
    "observation id"
);

opaque_id!(
    /// Identifier of an [`Element`](crate::Element), unique within an
    /// observation.
    ElementId,
    "element id"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_ids() {
        assert_eq!(ElementId::new(""), Err(Error::EmptyId { kind: "element id" }));
        assert!(serde_json::from_str::<ObservationId>(r#""""#).is_err());
    }

    #[test]
    fn serializes_as_plain_string() {
        let id = ElementId::new("e_1").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""e_1""#);
        assert_eq!(serde_json::from_str::<ElementId>(r#""e_1""#).unwrap(), id);
    }
}
