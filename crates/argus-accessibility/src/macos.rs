//! macOS accessibility backend (AXUIElement API).
//!
//! Each node is read with a single `AXUIElementCopyMultipleAttributeValues`
//! call (one IPC round trip). Traversal is bounded in node count and depth,
//! and every request has a messaging timeout, so an unresponsive application
//! cannot hang Argus.

use std::ptr::{self, NonNull};

use argus_protocol::Application;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElement, AXValue, AXValueType, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGSize,
};
use objc2_foundation::NSString;

use crate::{AccessibilityBackend, AppTarget, AxFrame, AxNode, AxSnapshot, AxValue, Error, Result};

/// Maximum number of nodes read from one window.
const MAX_NODES: usize = 5_000;

/// Maximum tree depth.
const MAX_DEPTH: usize = 64;

/// Seconds to wait for an application to answer one request.
const MESSAGING_TIMEOUT: f32 = 1.0;

/// Attributes read for every node, in the order of [`Attr`].
const ATTRIBUTES: [&str; 15] = [
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

        let mut reader = TreeReader { attributes: &self.attributes, nodes: 0, truncated: false };
        let window = reader.read(&window, 0).ok_or(Error::NoWindow)?;
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

fn resolve(target: &AppTarget) -> Result<Retained<NSRunningApplication>> {
    let workspace = NSWorkspace::sharedWorkspace();
    match target {
        AppTarget::Frontmost => workspace
            .frontmostApplication()
            .ok_or_else(|| Error::ApplicationNotFound("the frontmost application".to_owned())),
        AppTarget::Pid(pid) => i32::try_from(*pid)
            .ok()
            .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .ok_or_else(|| Error::ApplicationNotFound(format!("pid {pid}"))),
        AppTarget::Name(name) => {
            let wanted = name.to_lowercase();
            workspace
                .runningApplications()
                .iter()
                .find(|app| {
                    let matches = |text: Option<Retained<NSString>>| {
                        text.is_some_and(|text| text.to_string().to_lowercase() == wanted)
                    };
                    matches(app.localizedName()) || matches(app.bundleIdentifier())
                })
                .ok_or_else(|| Error::ApplicationNotFound(format!("`{name}`")))
        }
    }
}

/// The focused window, falling back to the main window and the first window.
fn window_of(app: &AXUIElement) -> Result<CFRetained<AXUIElement>> {
    for attribute in ["AXFocusedWindow", "AXMainWindow"] {
        match attribute_value(app, attribute) {
            Ok(Some(value)) => {
                if let Ok(window) = value.downcast::<AXUIElement>() {
                    return Ok(window);
                }
            }
            Ok(None) => {}
            Err(error) => return Err(error),
        }
    }
    let windows = attribute_value(app, "AXWindows")?
        .and_then(|value| value.downcast::<CFArray>().ok())
        .ok_or(Error::NoWindow)?;
    // SAFETY: `AXWindows` is an array of CF objects.
    let windows: &CFArray<CFType> = unsafe { windows.cast_unchecked() };
    windows.iter().find_map(|window| window.downcast::<AXUIElement>().ok()).ok_or(Error::NoWindow)
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
        self.nodes += 1;

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
            children: Vec::new(),
        };

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
