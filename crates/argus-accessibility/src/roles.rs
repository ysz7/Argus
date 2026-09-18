//! Mapping of macOS accessibility roles to protocol roles.

use argus_protocol::Role;

/// Maps a platform role and subrole to a protocol role.
///
/// Anything without a clear protocol equivalent maps to [`Role::Unknown`];
/// guessing would be worse than admitting ignorance.
pub(crate) fn map_role(role: &str, subrole: Option<&str>) -> Role {
    match (role, subrole) {
        ("AXWindow", Some("AXDialog" | "AXSystemDialog")) => Role::Dialog,
        ("AXWindow", _) => Role::Window,
        ("AXSheet" | "AXDrawer", _) => Role::Dialog,

        ("AXRadioButton", Some("AXTabButton")) => Role::Tab,
        ("AXRadioButton", _) => Role::RadioButton,
        ("AXCheckBox", _) => Role::Checkbox,
        ("AXButton" | "AXPopUpButton" | "AXMenuButton" | "AXDisclosureTriangle", _) => Role::Button,

        ("AXStaticText" | "AXHeading", _) => Role::Text,
        ("AXTextField" | "AXTextArea" | "AXComboBox", _) => Role::TextBox,

        ("AXMenu" | "AXMenuBar", _) => Role::Menu,
        ("AXMenuItem" | "AXMenuBarItem", _) => Role::MenuItem,

        ("AXList" | "AXOutline", _) => Role::List,
        ("AXTable" | "AXGrid", _) => Role::Table,
        ("AXRow", _) => Role::Row,
        ("AXCell", _) => Role::Cell,

        ("AXImage", _) => Role::Image,
        ("AXSlider", _) => Role::Slider,
        ("AXProgressIndicator" | "AXBusyIndicator", _) => Role::ProgressBar,
        ("AXLink", _) => Role::Link,

        (
            "AXGroup" | "AXSplitGroup" | "AXScrollArea" | "AXToolbar" | "AXTabGroup"
            | "AXRadioGroup" | "AXLayoutArea" | "AXWebArea" | "AXBrowser",
            _,
        ) => Role::Group,

        _ => Role::Unknown,
    }
}

/// Name of controls whose purpose is fully defined by their subrole (window
/// buttons carry no label of their own).
pub(crate) fn subrole_name(subrole: Option<&str>) -> Option<&'static str> {
    match subrole? {
        "AXCloseButton" => Some("Close"),
        "AXMinimizeButton" => Some("Minimize"),
        "AXZoomButton" => Some("Zoom"),
        "AXFullScreenButton" => Some("Full Screen"),
        _ => None,
    }
}

/// Whether the role clips its descendants to its own frame.
pub(crate) fn clips_children(role: &str) -> bool {
    matches!(role, "AXWindow" | "AXSheet" | "AXScrollArea")
}

/// Whether the platform role holds its visible text in the value attribute.
pub(crate) fn text_in_value(role: &str) -> bool {
    matches!(role, "AXStaticText" | "AXHeading")
}

/// Whether the platform role is an editable text input.
pub(crate) fn is_text_input(role: &str) -> bool {
    matches!(role, "AXTextField" | "AXTextArea" | "AXComboBox")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_controls() {
        let cases = [
            ("AXButton", None, Role::Button),
            ("AXButton", Some("AXCloseButton"), Role::Button),
            ("AXPopUpButton", None, Role::Button),
            ("AXCheckBox", Some("AXSwitch"), Role::Checkbox),
            ("AXRadioButton", None, Role::RadioButton),
            ("AXRadioButton", Some("AXTabButton"), Role::Tab),
            ("AXStaticText", None, Role::Text),
            ("AXTextField", Some("AXSearchField"), Role::TextBox),
            ("AXTextField", Some("AXSecureTextField"), Role::TextBox),
            ("AXWindow", Some("AXStandardWindow"), Role::Window),
            ("AXWindow", Some("AXDialog"), Role::Dialog),
            ("AXSheet", None, Role::Dialog),
            ("AXMenuItem", None, Role::MenuItem),
            ("AXOutline", None, Role::List),
            ("AXRow", Some("AXOutlineRow"), Role::Row),
            ("AXScrollArea", None, Role::Group),
            ("AXProgressIndicator", None, Role::ProgressBar),
        ];
        for (role, subrole, expected) in cases {
            assert_eq!(map_role(role, subrole), expected, "{role} / {subrole:?}");
        }
    }

    #[test]
    fn unfamiliar_roles_are_unknown() {
        for role in ["AXSplitter", "AXScrollBar", "AXColorWell", "AXValueIndicator", "AXFoo", ""] {
            assert_eq!(map_role(role, None), Role::Unknown, "{role}");
        }
    }
}
