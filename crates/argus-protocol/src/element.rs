use serde::{Deserialize, Serialize};

use crate::{Bounds, Confidence, ElementId, Source};

/// A single user-interface element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// Identifier, unique within the observation.
    pub id: ElementId,
    /// Semantic role.
    pub role: Role,
    /// Accessible name or visible label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Current value (text field contents, slider position, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Longer description or help text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Selection and styling of `value`, for text controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextState>,
    /// Full extent of the element, including parts that are clipped.
    pub bounds: Bounds,
    /// The part of `bounds` that is actually visible on screen, after
    /// clipping by scroll views, windows and display edges. `None` means
    /// unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_bounds: Option<Bounds>,
    /// Interaction state.
    #[serde(default, skip_serializing_if = "ElementState::is_unknown")]
    pub state: ElementState,
    /// Property-level confidence.
    pub confidence: Confidence,
    /// Sources that contributed evidence for this element.
    pub sources: Vec<Source>,
    /// Structural parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ElementId>,
    /// Structural children, in reading order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ElementId>,
}

/// Semantic role of an element.
///
/// [`Role::Unknown`] is a normal, honest result: it means Argus could not
/// determine the role with any useful confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Role {
    Window,
    Dialog,
    Group,
    Button,
    Text,
    TextBox,
    Checkbox,
    RadioButton,
    Menu,
    MenuItem,
    Tab,
    List,
    ListItem,
    Table,
    Row,
    Cell,
    Image,
    Icon,
    Slider,
    ProgressBar,
    Link,
    /// The role is unknown, or is not known to this protocol version.
    #[serde(other)]
    Unknown,
}

/// Interaction state of an element.
///
/// Every property is optional because *unknown is not false*: `None` means
/// Argus has no evidence either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementState {
    /// The element accepts interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// At least part of the element is visible on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// The element has keyboard focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
    /// The element is selected (list item, tab, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// Check state of checkboxes, radio buttons and toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<CheckState>,
    /// The element is expanded (disclosure, tree node, combo box).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// The element's value can be edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editable: Option<bool>,
}

impl ElementState {
    /// Whether nothing is known about the state.
    pub fn is_unknown(&self) -> bool {
        *self == Self::default()
    }
}

/// Selection and styling of the text in an element's `value`.
///
/// Positions count Unicode scalar values (characters) of `value`, from 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextState {
    /// The selected range; an empty range is the insertion point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TextRange>,
    /// Consecutive ranges of uniform style, covering the text in order.
    /// Empty when the style is unknown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<TextRun>,
}

impl TextState {
    /// Whether nothing is known.
    pub fn is_empty(&self) -> bool {
        self.selection.is_none() && self.runs.is_empty()
    }
}

/// A range of characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    /// First character.
    pub start: u32,
    /// Number of characters.
    pub length: u32,
}

/// A range of text with one style. Unknown style properties are omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    /// First character.
    pub start: u32,
    /// Number of characters.
    pub length: u32,
    /// Font name, e.g. `Helvetica-Bold`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Font size in points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// Bold weight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    /// Italic or oblique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    /// Underlined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
}

impl TextRun {
    /// Whether two runs have the same style.
    pub fn same_style(&self, other: &Self) -> bool {
        self.font == other.font
            && self.size == other.size
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
    }
}

/// Check state of a checkable element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// Checked / on.
    Checked,
    /// Unchecked / off.
    Unchecked,
    /// Indeterminate (e.g. a parent checkbox with partially checked children).
    Mixed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_use_snake_case() {
        assert_eq!(serde_json::to_string(&Role::TextBox).unwrap(), r#""text_box""#);
        assert_eq!(serde_json::to_string(&Role::ProgressBar).unwrap(), r#""progress_bar""#);
    }

    #[test]
    fn unknown_role_parses_as_unknown() {
        assert_eq!(serde_json::from_str::<Role>(r#""color_well""#).unwrap(), Role::Unknown);
    }

    #[test]
    fn missing_state_is_unknown_not_false() {
        let state: ElementState = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert_eq!(state.enabled, Some(false));
        assert_eq!(state.visible, None);
        assert_eq!(serde_json::to_string(&ElementState::default()).unwrap(), "{}");
    }

    #[test]
    fn rejects_invalid_check_state() {
        assert!(serde_json::from_str::<CheckState>(r#""maybe""#).is_err());
        assert!(serde_json::from_str::<CheckState>("true").is_err());
    }
}
