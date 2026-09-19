use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Bounds, Element, ObservationId, PROTOCOL_VERSION, Relation};

/// A structured description of the user interface at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Protocol version the observation conforms to, e.g. `"0.1"`.
    pub protocol_version: String,
    /// Identifier of this observation.
    pub id: ObservationId,
    /// When the observed state was captured.
    pub timestamp: Timestamp,
    /// The earlier observation of the same tracking session whose element
    /// IDs this observation continues. `None` when tracking started with
    /// this observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<ObservationId>,
    /// The observed application, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<Application>,
    /// The observed window, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    /// All observed elements, parents before children.
    pub elements: Vec<Element>,
    /// Non-hierarchical relations between elements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
}

impl Observation {
    /// Creates an empty observation for the current protocol version.
    pub fn new(id: ObservationId, timestamp: Timestamp) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            id,
            timestamp,
            previous: None,
            application: None,
            window: None,
            elements: Vec::new(),
            relations: Vec::new(),
        }
    }
}

/// Wall-clock time in milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// The current wall-clock time.
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        Self(u64::try_from(millis).unwrap_or(u64::MAX))
    }
}

/// The application that owns the observed interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Application {
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Platform bundle identifier (e.g. `com.apple.calculator`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// Process identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// The observed window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// Window title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Window frame in global screen coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,
}
