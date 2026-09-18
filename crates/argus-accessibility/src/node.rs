use argus_protocol::Application;
use serde::{Deserialize, Serialize};

/// A raw accessibility tree of one window, as read from the platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxSnapshot {
    /// The application owning the window.
    pub application: Application,
    /// The window node; its descendants form the tree.
    pub window: AxNode,
    /// Whether traversal stopped early because of size or depth limits.
    #[serde(default)]
    pub truncated: bool,
}

/// One raw accessibility node with platform vocabulary (e.g. `AXButton`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AxNode {
    /// Platform role, e.g. `AXButton`.
    pub role: String,
    /// Platform subrole, e.g. `AXCloseButton`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    /// Title (usually the visible label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Accessibility description (label of unlabelled controls).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Help text (tooltip).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Placeholder of an empty text field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Developer-assigned identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// Raw value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<AxValue>,
    /// Frame in global screen points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<AxFrame>,
    /// `AXEnabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `AXFocused`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
    /// `AXSelected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// `AXExpanded`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// Child nodes in platform order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AxNode>,
}

/// A raw accessibility value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AxValue {
    /// A boolean.
    Bool(bool),
    /// A number (check state, slider position, ...).
    Number(f64),
    /// Text.
    String(String),
}

/// A rectangle in global screen points, as reported by the platform.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AxFrame {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}
