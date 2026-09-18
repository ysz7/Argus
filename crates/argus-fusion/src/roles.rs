//! Role vocabulary used by the fusion policy.

use argus_protocol::Role;

/// Elements whose visible text is their name or value: text recognized
/// inside them describes them.
pub(crate) fn carries_text(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Checkbox
            | Role::RadioButton
            | Role::Tab
            | Role::Link
            | Role::MenuItem
            | Role::ListItem
            | Role::Cell
            | Role::Text
            | Role::TextBox
    )
}

/// Leaf controls. A visual detection inside one of them describes the control
/// (or a detail of it), not a separate object; their frames often include a
/// label, so containment counts as a match.
pub(crate) fn is_control(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Checkbox
            | Role::RadioButton
            | Role::Tab
            | Role::Link
            | Role::MenuItem
            | Role::TextBox
            | Role::Slider
            | Role::Icon
            | Role::Image
            | Role::ListItem
            | Role::Cell
    )
}

/// Pixel-only elements that take the text recognized on them as their name.
pub(crate) fn named_by_text(role: Role) -> bool {
    matches!(
        role,
        Role::Button | Role::Tab | Role::Link | Role::MenuItem | Role::ListItem | Role::Icon
    )
}

/// Visual hypotheses that may be just a glyph or a detail of something else.
pub(crate) fn is_detail(role: Role) -> bool {
    matches!(role, Role::Icon | Role::Unknown)
}

/// Roles whose check state is meaningful (same rule as normalization).
pub(crate) fn is_checkable(role: Role) -> bool {
    matches!(role, Role::Checkbox | Role::RadioButton | Role::MenuItem | Role::Unknown)
}

/// Whether a visual role hypothesis is consistent with the role another
/// source reports, at the resolution of visual detection: a detector sees
/// the same rounded rectangle for a button, a link-styled button or a menu
/// item.
pub(crate) fn compatible(visual: Role, other: Role) -> bool {
    use Role::*;
    if visual == other || visual == Unknown || other == Unknown {
        return true;
    }
    match visual {
        Button => {
            matches!(other, Tab | Link | MenuItem | RadioButton | ListItem | Cell | Icon | Image)
        }
        Tab => matches!(other, Button | RadioButton | ListItem),
        Checkbox | RadioButton => matches!(other, Checkbox | RadioButton),
        Icon => matches!(other, Button | Image | Link | MenuItem | Tab | ListItem | Cell),
        Group => matches!(other, List | Table | Row | Dialog),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_roles_are_coarse() {
        assert!(compatible(Role::Button, Role::Link));
        assert!(compatible(Role::Checkbox, Role::RadioButton));
        assert!(compatible(Role::Unknown, Role::Slider));
        assert!(!compatible(Role::TextBox, Role::Button));
        assert!(!compatible(Role::Checkbox, Role::Button));
    }
}
