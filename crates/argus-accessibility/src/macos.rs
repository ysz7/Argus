//! macOS accessibility backend (AXUIElement API).
//!
//! Each node is read with a single `AXUIElementCopyMultipleAttributeValues`
//! call (one IPC round trip). Traversal is bounded in node count and depth,
//! and every request has a messaging timeout, so an unresponsive application
//! cannot hang Argus.

use std::ptr::{self, NonNull};

use argus_protocol::Application;
use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;
use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElement, AXValue, AXValueType, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{
    CFArray, CFAttributedString, CFBoolean, CFDictionary, CFNumber, CFRange, CFRetained, CFString,
    CFType, CGPoint, CGSize, Type,
};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowLayer, kCGWindowOwnerName,
    kCGWindowOwnerPID,
};
use objc2_foundation::NSString;

use crate::{
    AccessibilityBackend, AppTarget, AxFrame, AxNode, AxRange, AxRun, AxSnapshot, AxValue, Error,
    Result,
};

/// Maximum number of nodes read from one window.
const MAX_NODES: usize = 5_000;

/// Maximum tree depth.
const MAX_DEPTH: usize = 64;

/// Seconds to wait for an application to answer one request.
const MESSAGING_TIMEOUT: f32 = 1.0;

/// Attributes read for every node, in the order of [`Attr`].
const ATTRIBUTES: [&str; 17] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXHelp",
    "AXPlaceholderValue",
    "AXIdentifier",
    "AXValue",
    "AXPosition",
    "AXSize",
    "AXEnabled",
    "AXFocused",
    "AXSelected",
    "AXExpanded",
    "AXChildren",
    "AXTitleUIElement",
    "AXSelectedTextRange",
];

/// Indices into [`ATTRIBUTES`].
#[derive(Clone, Copy)]
enum Attr {
    Role,
    Subrole,
    Title,
    Description,
    Help,
    Placeholder,
    Identifier,
    Value,
    Position,
    Size,
    Enabled,
    Focused,
    Selected,
    Expanded,
    Children,
    TitleElement,
    SelectedTextRange,
}

/// Accessibility backend for macOS.
#[derive(Debug)]
pub struct MacAccessibilityBackend {
    attributes: CFRetained<CFArray<CFString>>,
}

impl MacAccessibilityBackend {
    /// Creates the backend.
    pub fn new() -> Self {
        let names: Vec<_> = ATTRIBUTES.iter().map(|name| CFString::from_static_str(name)).collect();
        Self { attributes: CFArray::from_retained_objects(&names) }
    }
}

impl Default for MacAccessibilityBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AccessibilityBackend for MacAccessibilityBackend {
    fn has_permission(&self) -> bool {
        // SAFETY: no arguments; reads the process's trust status.
        unsafe { AXIsProcessTrusted() }
    }

    fn snapshot(&self, target: &AppTarget) -> Result<AxSnapshot> {
        ensure_permission()?;
        let application = resolve(target)?;
        let pid = application.processIdentifier();

        // SAFETY: creating an element reference for a process id has no
        // preconditions; the system-wide element configures the timeout for
        // every request of this process.
        let app_element = unsafe {
            AXUIElement::new_system_wide().set_messaging_timeout(MESSAGING_TIMEOUT);
            AXUIElement::new_application(pid)
        };
        let window = window_of(&app_element)?;

        let mut reader = TreeReader {
            attributes: &self.attributes,
            nodes: 0,
            truncated: false,
            elements: Vec::new(),
            titles: Vec::new(),
        };
        let mut window = reader.read(&window, 0).ok_or(Error::NoWindow)?;
        reader.resolve_labels(&mut window);
        let menus =
            open_menus(&app_element).iter().filter_map(|menu| reader.read(menu, 1)).collect();
        tracing::debug!(
            nodes = reader.nodes,
            truncated = reader.truncated,
            "read accessibility tree"
        );

        Ok(AxSnapshot {
            application: Application {
                name: application.localizedName().map(|name| name.to_string()),
                bundle_id: application.bundleIdentifier().map(|id| id.to_string()),
                pid: u32::try_from(pid).ok(),
            },
            window,
            menus,
            truncated: reader.truncated,
        })
    }
}

/// Checks the Accessibility permission, asking the system to prompt the user
/// when it has not been granted yet.
fn ensure_permission() -> Result<()> {
    // SAFETY: no arguments; reads the process's trust status.
    if unsafe { AXIsProcessTrusted() } {
        return Ok(());
    }
    // SAFETY: `kAXTrustedCheckOptionPrompt` is an immutable constant.
    let key = unsafe { kAXTrustedCheckOptionPrompt };
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
    // SAFETY: `options` is a valid dictionary with the documented key.
    if unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) } {
        Ok(())
    } else {
        Err(Error::PermissionDenied)
    }
}

/// Finds the target application.
///
/// `NSWorkspace`'s `runningApplications` and `frontmostApplication` are
/// updated through the main run loop and go stale in processes without one
/// (CLI tools on background threads, daemons). Only lookups that query the
/// system on every call are used here.
fn resolve(target: &AppTarget) -> Result<Retained<NSRunningApplication>> {
    match target {
        AppTarget::Frontmost => focused_application_pid()
            .or_else(topmost_window_pid)
            .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .ok_or_else(|| Error::ApplicationNotFound("the frontmost application".to_owned())),
        AppTarget::Pid(pid) => i32::try_from(*pid)
            .ok()
            .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .ok_or_else(|| Error::ApplicationNotFound(format!("pid {pid}"))),
        AppTarget::Name(name) => {
            let by_bundle_id = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(name),
            );
            by_bundle_id
                .firstObject()
                .or_else(|| {
                    window_owner_pid(name)
                        .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
                })
                .ok_or_else(|| Error::ApplicationNotFound(format!("`{name}`")))
        }
    }
}

/// Process id of the application with keyboard focus, as reported by the
/// accessibility system. Fails for some focused applications (e.g. Electron
/// apps without accessibility enabled).
fn focused_application_pid() -> Option<i32> {
    // SAFETY: creating the system-wide element has no preconditions.
    let system = unsafe { AXUIElement::new_system_wide() };
    let app = match attribute_value(&system, "AXFocusedApplication") {
        Ok(app) => app?.downcast::<AXUIElement>().ok()?,
        Err(error) => {
            tracing::debug!(%error, "focused application unavailable; using the window list");
            return None;
        }
    };
    let mut pid = 0;
    // SAFETY: `pid` is a valid out-pointer.
    (unsafe { app.pid(NonNull::from(&mut pid)) } == AXError::Success).then_some(pid)
}

/// Process id of the owner of the frontmost normal (layer 0) window.
fn topmost_window_pid() -> Option<i32> {
    window_owners(true).find(|owner| owner.layer == 0).map(|owner| owner.pid)
}

/// Process id of an application with an on-screen window whose owner name
/// matches `name` (case-insensitive).
fn window_owner_pid(name: &str) -> Option<i32> {
    let wanted = name.to_lowercase();
    // All windows, not only on-screen ones: an application whose windows are
    // on another Space is still running and readable.
    window_owners(false).find(|owner| owner.name.to_lowercase() == wanted).map(|owner| owner.pid)
}

struct WindowOwner {
    pid: i32,
    name: String,
    layer: i64,
}

/// Owners of windows (only on-screen ones if `on_screen`), front to back.
fn window_owners(on_screen: bool) -> impl Iterator<Item = WindowOwner> {
    let options = match on_screen {
        true => CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        false => CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
    };
    let list = CGWindowListCopyWindowInfo(options, 0);
    // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries with
    // string keys.
    let list: Vec<_> = list
        .map(|list| unsafe { list.cast_unchecked::<CFDictionary<CFString, CFType>>() }.to_vec())
        .unwrap_or_default();
    // SAFETY: the `kCGWindow*` keys are immutable constants.
    let (owner_key, pid_key, layer_key) =
        unsafe { (kCGWindowOwnerName, kCGWindowOwnerPID, kCGWindowLayer) };
    list.into_iter().filter_map(move |info| {
        let number = |key| info.get(key)?.downcast::<CFNumber>().ok()?.as_i64();
        Some(WindowOwner {
            pid: i32::try_from(number(pid_key)?).ok()?,
            name: info.get(owner_key)?.downcast::<CFString>().ok()?.to_string(),
            layer: number(layer_key)?,
        })
    })
}

/// The focused window, falling back to the main window and the first window.
///
/// Only elements with the window role count: some applications that were
/// never activated answer `AXFocusedWindow` with the application element.
fn window_of(app: &AXUIElement) -> Result<CFRetained<AXUIElement>> {
    for attribute in ["AXFocusedWindow", "AXMainWindow"] {
        if let Some(value) = attribute_value(app, attribute)? {
            if let Ok(window) = value.downcast::<AXUIElement>() {
                if is_window(&window)? {
                    return Ok(window);
                }
                tracing::debug!(attribute, "ignoring an element that is not a window");
            }
        }
    }
    let windows = attribute_value(app, "AXWindows")?
        .and_then(|value| value.downcast::<CFArray>().ok())
        .ok_or(Error::NoWindow)?;
    // SAFETY: `AXWindows` is an array of CF objects.
    let windows: &CFArray<CFType> = unsafe { windows.cast_unchecked() };
    for window in windows.iter() {
        if let Ok(window) = window.downcast::<AXUIElement>() {
            if is_window(&window)? {
                return Ok(window);
            }
        }
    }
    Err(Error::NoWindow)
}

/// Menus the application has open outside its windows: context menus
/// (children of the application element) and the menu of the selected
/// menu-bar item.
fn open_menus(app: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    let children = |element: &AXUIElement| -> Vec<CFRetained<AXUIElement>> {
        let Ok(Some(value)) = attribute_value(element, "AXChildren") else { return Vec::new() };
        let Ok(array) = value.downcast::<CFArray>() else { return Vec::new() };
        // SAFETY: `AXChildren` is an array of CF objects.
        let array: &CFArray<CFType> = unsafe { array.cast_unchecked() };
        array.iter().filter_map(|child| child.downcast::<AXUIElement>().ok()).collect()
    };
    let role = |element: &AXUIElement| {
        attribute_value(element, "AXRole").ok().flatten().as_deref().and_then(cf_string)
    };
    let mut menus = Vec::new();
    for child in children(app) {
        match role(&child).as_deref() {
            Some("AXMenu") => menus.push(child),
            Some("AXMenuBar") => {
                for item in children(&child) {
                    let selected = attribute_value(&item, "AXSelected")
                        .ok()
                        .flatten()
                        .as_deref()
                        .and_then(cf_bool)
                        .unwrap_or(false);
                    if selected {
                        menus.extend(
                            children(&item)
                                .into_iter()
                                .filter(|menu| role(menu).as_deref() == Some("AXMenu")),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    menus.truncate(4);
    menus
}

fn is_window(element: &AXUIElement) -> Result<bool> {
    let role = attribute_value(element, "AXRole")?;
    Ok(role.as_deref().and_then(cf_string).is_some_and(|role| role == "AXWindow"))
}

fn attribute_value(
    element: &AXUIElement,
    name: &'static str,
) -> Result<Option<CFRetained<CFType>>> {
    let mut value: *const CFType = ptr::null();
    // SAFETY: `value` is a valid out-pointer; on success it receives a +1
    // reference that is adopted below.
    let status = unsafe {
        element.copy_attribute_value(&CFString::from_static_str(name), NonNull::from(&mut value))
    };
    match status {
        AXError::Success => Ok(NonNull::new(value.cast_mut())
            // SAFETY: the copied value is owned by us (+1).
            .map(|value| unsafe { CFRetained::from_raw(value) })),
        AXError::APIDisabled => Err(Error::PermissionDenied),
        AXError::NoValue | AXError::AttributeUnsupported => Ok(None),
        AXError::CannotComplete => Err(Error::Platform(
            "the application did not respond to accessibility requests".to_owned(),
        )),
        other => Err(Error::Platform(format!("reading {name} failed: AXError {}", other.0))),
    }
}

struct TreeReader<'a> {
    attributes: &'a CFArray<CFString>,
    nodes: usize,
    truncated: bool,
    /// Every element read, in pre-order (index = node index).
    elements: Vec<CFRetained<AXUIElement>>,
    /// `(node index, its AXTitleUIElement)`, resolved after the traversal.
    titles: Vec<(usize, CFRetained<AXUIElement>)>,
}

impl TreeReader<'_> {
    /// Reads `element` and its descendants; `None` if the element does not
    /// answer.
    fn read(&mut self, element: &AXUIElement, depth: usize) -> Option<AxNode> {
        if self.nodes >= MAX_NODES || depth > MAX_DEPTH {
            self.truncated = true;
            return None;
        }
        let values = self.values(element)?;
        let index = self.nodes;
        self.nodes += 1;
        self.elements.push(element.retain());

        let get = |attr: Attr| {
            values.get(attr as usize).map(|value| &**value).filter(|value| present(value))
        };
        let string = |attr: Attr| get(attr).and_then(cf_string);
        let boolean = |attr: Attr| get(attr).and_then(cf_bool);

        let mut node = AxNode {
            role: string(Attr::Role).unwrap_or_default(),
            subrole: string(Attr::Subrole),
            title: string(Attr::Title),
            description: string(Attr::Description),
            help: string(Attr::Help),
            placeholder: string(Attr::Placeholder),
            identifier: string(Attr::Identifier),
            value: get(Attr::Value).and_then(ax_value),
            frame: frame(get(Attr::Position), get(Attr::Size)),
            enabled: boolean(Attr::Enabled),
            focused: boolean(Attr::Focused),
            selected: boolean(Attr::Selected),
            expanded: boolean(Attr::Expanded),
            selection: get(Attr::SelectedTextRange).and_then(range),
            runs: Vec::new(),
            label: None,
            children: Vec::new(),
        };
        if matches!(node.role.as_str(), "AXTextArea" | "AXTextField")
            && let Some(AxValue::String(text)) = &node.value
        {
            node.runs = runs(element, text.encode_utf16().count());
        }
        if let Some(title) =
            get(Attr::TitleElement).and_then(|value| value.downcast_ref::<AXUIElement>())
        {
            self.titles.push((index, title.retain()));
        }

        if let Some(children) =
            get(Attr::Children).and_then(|value| value.downcast_ref::<CFArray>())
        {
            // SAFETY: `AXChildren` is an array of CF objects.
            let children: &CFArray<CFType> = unsafe { children.cast_unchecked() };
            for child in children.iter() {
                if let Ok(child) = child.downcast::<AXUIElement>()
                    && let Some(child) = self.read(&child, depth + 1)
                {
                    node.children.push(child);
                }
            }
        }
        Some(node)
    }

    /// Points every node that has a title element at that element's node,
    /// if the element is part of the tree.
    fn resolve_labels(&self, root: &mut AxNode) {
        let mut labels = vec![None; self.nodes];
        for (index, title) in &self.titles {
            labels[*index] = self.elements.iter().position(|element| **element == **title);
        }
        let mut next = 0;
        assign_labels(root, &labels, &mut next);
    }

    fn values(&self, element: &AXUIElement) -> Option<Vec<CFRetained<CFType>>> {
        let mut values: *const CFArray = ptr::null();
        // SAFETY: the attribute array holds CFStrings and `values` is a valid
        // out-pointer that receives a +1 reference on success.
        let status = unsafe {
            element.copy_multiple_attribute_values(
                self.attributes.as_opaque(),
                AXCopyMultipleAttributeOptions::empty(),
                NonNull::from(&mut values),
            )
        };
        if status != AXError::Success {
            tracing::trace!(status = status.0, "skipping unresponsive accessibility element");
            return None;
        }
        // SAFETY: on success `values` is an owned (+1) array of CF objects.
        let values: CFRetained<CFArray> =
            unsafe { CFRetained::from_raw(NonNull::new(values.cast_mut())?) };
        // SAFETY: the result holds one CF object per requested attribute.
        let values: &CFArray<CFType> = unsafe { values.cast_unchecked() };
        Some(values.to_vec())
    }
}

/// Sets `label` on every node in pre-order (`next` is the running index).
fn assign_labels(node: &mut AxNode, labels: &[Option<usize>], next: &mut usize) {
    node.label = labels.get(*next).copied().flatten();
    *next += 1;
    for child in &mut node.children {
        assign_labels(child, labels, next);
    }
}

/// Missing attributes are reported as `AXValue`s wrapping an `AXError`.
fn present(value: &CFType) -> bool {
    match value.downcast_ref::<AXValue>() {
        Some(ax) => {
            // SAFETY: reading the type of a valid AXValue.
            let kind = unsafe { ax.r#type() };
            kind != AXValueType::AXError
        }
        None => true,
    }
}

fn cf_string(value: &CFType) -> Option<String> {
    value.downcast_ref::<CFString>().map(|text| text.to_string())
}

fn cf_bool(value: &CFType) -> Option<bool> {
    if let Some(boolean) = value.downcast_ref::<CFBoolean>() {
        return Some(boolean.as_bool());
    }
    value.downcast_ref::<CFNumber>()?.as_i64().map(|number| number != 0)
}

fn ax_value(value: &CFType) -> Option<AxValue> {
    if let Some(boolean) = value.downcast_ref::<CFBoolean>() {
        return Some(AxValue::Bool(boolean.as_bool()));
    }
    if let Some(number) = value.downcast_ref::<CFNumber>() {
        return number.as_f64().map(AxValue::Number);
    }
    cf_string(value).map(AxValue::String)
}

/// Longest text whose style runs are read, in UTF-16 code units.
const MAX_STYLED_TEXT: usize = 20_000;

fn range(value: &CFType) -> Option<AxRange> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut range = CFRange { location: 0, length: 0 };
    // SAFETY: the out-pointer matches the requested type; `value` returns
    // false if the AXValue holds a different type.
    let ok = unsafe { value.value(AXValueType::CFRange, NonNull::from(&mut range).cast()) };
    ok.then(|| AxRange {
        location: u32::try_from(range.location).unwrap_or(0),
        length: u32::try_from(range.length).unwrap_or(0),
    })
}

/// Style runs of the first `length` UTF-16 units of a text control.
fn runs(element: &AXUIElement, length: usize) -> Vec<AxRun> {
    if length == 0 || length > MAX_STYLED_TEXT {
        return Vec::new();
    }
    let mut whole = CFRange { location: 0, length: length as isize };
    // SAFETY: `whole` is a valid CFRange for the duration of the call.
    let Some(parameter) =
        (unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&mut whole).cast()) })
    else {
        return Vec::new();
    };
    let mut result: *const CFType = ptr::null();
    // SAFETY: the parameter is a CFRange AXValue, as the attribute expects;
    // `result` receives a +1 reference on success.
    let status = unsafe {
        element.copy_parameterized_attribute_value(
            &CFString::from_static_str("AXAttributedStringForRange"),
            &parameter,
            NonNull::from(&mut result),
        )
    };
    if status != AXError::Success {
        return Vec::new();
    }
    let Some(result) = NonNull::new(result.cast_mut()) else { return Vec::new() };
    // SAFETY: the copied value is owned by us (+1).
    let result: CFRetained<CFType> = unsafe { CFRetained::from_raw(result) };
    let Ok(text) = result.downcast::<CFAttributedString>() else { return Vec::new() };
    let total = text.length();
    let (font_key, underline_key, name_key, size_key) = (
        CFString::from_static_str("AXFont"),
        CFString::from_static_str("AXUnderline"),
        CFString::from_static_str("AXFontName"),
        CFString::from_static_str("AXFontSize"),
    );
    let mut runs = Vec::new();
    let mut location = 0;
    while location < total && runs.len() < 1_000 {
        let mut effective = CFRange { location: 0, length: 0 };
        let whole = CFRange { location: 0, length: total };
        // SAFETY: `location` is inside the string and `effective` is a valid
        // out-pointer.
        let attributes =
            unsafe { text.attributes_and_longest_effective_range(location, whole, &mut effective) };
        if effective.length <= 0 {
            break;
        }
        let mut run = AxRun {
            range: AxRange {
                location: u32::try_from(effective.location).unwrap_or(0),
                length: u32::try_from(effective.length).unwrap_or(0),
            },
            font: None,
            size: None,
            underline: None,
        };
        if let Some(attributes) = attributes {
            // SAFETY: attributed string attributes are keyed by strings.
            let attributes: &CFDictionary<CFString, CFType> =
                unsafe { attributes.cast_unchecked() };
            if let Some(font) = attributes.get(&font_key)
                && let Ok(font) = font.downcast::<CFDictionary>()
            {
                // SAFETY: the font attribute is a dictionary keyed by strings.
                let font: &CFDictionary<CFString, CFType> = unsafe { font.cast_unchecked() };
                run.font = font.get(&name_key).as_deref().and_then(cf_string);
                run.size = font
                    .get(&size_key)
                    .and_then(|size| size.downcast::<CFNumber>().ok())
                    .and_then(|size| size.as_f64());
            }
            run.underline = attributes.get(&underline_key).as_deref().and_then(cf_bool);
        }
        runs.push(run);
        location = effective.location + effective.length;
    }
    runs
}

fn frame(position: Option<&CFType>, size: Option<&CFType>) -> Option<AxFrame> {
    let mut point = CGPoint::ZERO;
    let mut extent = CGSize::ZERO;
    let position = position?.downcast_ref::<AXValue>()?;
    let size = size?.downcast_ref::<AXValue>()?;
    // SAFETY: the out-pointers match the requested value types; `value`
    // returns false if the AXValue holds a different type.
    let ok = unsafe {
        position.value(AXValueType::CGPoint, NonNull::from(&mut point).cast())
            && size.value(AXValueType::CGSize, NonNull::from(&mut extent).cast())
    };
    ok.then_some(AxFrame { x: point.x, y: point.y, width: extent.width, height: extent.height })
}
