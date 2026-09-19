//! macOS input: Core Graphics events posted at the HID level.

use std::thread::sleep;
use std::time::Duration;

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_application_services::{AXIsProcessTrusted, AXUIElement};
use objc2_core_foundation::{CFBoolean, CFRetained, CFString, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventType, CGMainDisplayID, CGMouseButton, CGScrollEventUnit,
};

use crate::{Button, Combo, Error, Modifiers, Point, Result};

/// Pause after every event: applications drop events that come too fast.
const EVENT_GAP: Duration = Duration::from_millis(8);
/// Pause between moving the mouse and pressing a button.
const HOVER: Duration = Duration::from_millis(40);
/// Typed characters between checks that the user left the mouse alone.
const CHECK_EVERY: usize = 16;
/// How far the mouse may drift before an action counts as interrupted.
const TOLERANCE: f64 = 3.0;

pub(crate) fn trusted() -> bool {
    // SAFETY: a plain query without arguments.
    unsafe { AXIsProcessTrusted() }
}

#[derive(Debug)]
pub(crate) struct MacInput {
    source: CFRetained<CGEventSource>,
}

impl MacInput {
    pub(crate) fn new() -> Result<Self> {
        // Connects the process to the window server.
        let _ = CGMainDisplayID();
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .ok_or_else(|| Error::Platform("no event source".to_owned()))?;
        Ok(Self { source })
    }

    fn post(&self, event: Option<CFRetained<CGEvent>>) -> Result<()> {
        let event = event.ok_or_else(|| Error::Platform("the event was not created".to_owned()))?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        sleep(EVENT_GAP);
        Ok(())
    }

    fn mouse_event(
        &self,
        kind: CGEventType,
        point: Point,
        button: CGMouseButton,
    ) -> Option<CFRetained<CGEvent>> {
        CGEvent::new_mouse_event(Some(&self.source), kind, cg(point), button)
    }

    pub(crate) fn mouse(&self) -> Point {
        let event = CGEvent::new(None);
        let location = CGEvent::location(event.as_deref());
        Point::new(location.x, location.y)
    }

    pub(crate) fn move_to(&self, point: Point) -> Result<()> {
        self.post(self.mouse_event(CGEventType::MouseMoved, point, CGMouseButton::Left))
    }

    pub(crate) fn click(
        &self,
        point: Point,
        button: Button,
        count: u32,
        modifiers: Modifiers,
    ) -> Result<()> {
        let (down, up, cg_button) = match button {
            Button::Left => {
                (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left)
            }
            Button::Right => {
                (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right)
            }
            Button::Middle => {
                (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center)
            }
        };
        self.move_to(point)?;
        sleep(HOVER);
        let flags = flags(modifiers);
        for n in 1..=count {
            for kind in [down, up] {
                let event = self.mouse_event(kind, point, cg_button);
                CGEvent::set_integer_value_field(
                    event.as_deref(),
                    CGEventField::MouseEventClickState,
                    i64::from(n),
                );
                CGEvent::set_flags(event.as_deref(), flags);
                self.post(event)?;
            }
        }
        Ok(())
    }

    pub(crate) fn drag(&self, from: Point, to: Point) -> Result<()> {
        self.move_to(from)?;
        sleep(HOVER);
        self.post(self.mouse_event(CGEventType::LeftMouseDown, from, CGMouseButton::Left))?;
        for step in 1..=12 {
            let t = f64::from(step) / 12.0;
            let point = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.post(self.mouse_event(CGEventType::LeftMouseDragged, point, CGMouseButton::Left))?;
        }
        self.post(self.mouse_event(CGEventType::LeftMouseUp, to, CGMouseButton::Left))
    }

    pub(crate) fn scroll(&self, point: Point, dx: i32, dy: i32) -> Result<()> {
        self.move_to(point)?;
        self.post(CGEvent::new_scroll_wheel_event2(
            Some(&self.source),
            CGScrollEventUnit::Line,
            2,
            dy,
            dx,
            0,
        ))
    }

    pub(crate) fn type_text(&self, text: &str) -> Result<()> {
        let start = self.mouse();
        for (index, character) in text.chars().enumerate() {
            if index % CHECK_EVERY == 0 && index > 0 && self.mouse().moved_from(start, TOLERANCE) {
                return Err(Error::Interrupted);
            }
            if character == '\n' || character == '\r' {
                self.key(36, Modifiers::default())?;
                continue;
            }
            let mut units = [0u16; 2];
            let units = character.encode_utf16(&mut units);
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(Some(&self.source), 0, down);
                // SAFETY: `units` is a valid buffer of `units.len()` UTF-16
                // code units that outlives the call.
                unsafe {
                    CGEvent::keyboard_set_unicode_string(
                        event.as_deref(),
                        units.len() as _,
                        units.as_ptr(),
                    );
                }
                self.post(event)?;
            }
        }
        Ok(())
    }

    fn key(&self, code: u16, modifiers: Modifiers) -> Result<()> {
        let flags = flags(modifiers);
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(Some(&self.source), code, down);
            CGEvent::set_flags(event.as_deref(), flags);
            self.post(event)?;
        }
        Ok(())
    }

    pub(crate) fn press(&self, combo: &Combo, repeat: u32) -> Result<()> {
        for _ in 0..repeat {
            self.key(combo.code, combo.modifiers)?;
        }
        Ok(())
    }

    pub(crate) fn activate(&self, pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else { return false };
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            if app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
                return true;
            }
        }
        // Some processes (Tk applications) are not known to AppKit by their
        // pid; the accessibility API can still raise them.
        // SAFETY: any pid is accepted; an unknown one yields an element whose
        // calls fail.
        let app = unsafe { AXUIElement::new_application(pid) };
        let attribute = CFString::from_static_str("AXFrontmost");
        // SAFETY: `AXFrontmost` takes a boolean.
        let error = unsafe { app.set_attribute_value(&attribute, CFBoolean::new(true)) };
        error.0 == 0
    }
}

fn cg(point: Point) -> CGPoint {
    CGPoint { x: point.x, y: point.y }
}

fn flags(modifiers: Modifiers) -> CGEventFlags {
    let mut flags = CGEventFlags::empty();
    for (held, flag) in [
        (modifiers.cmd, CGEventFlags::MaskCommand),
        (modifiers.shift, CGEventFlags::MaskShift),
        (modifiers.alt, CGEventFlags::MaskAlternate),
        (modifiers.ctrl, CGEventFlags::MaskControl),
        (modifiers.function, CGEventFlags::MaskSecondaryFn),
    ] {
        if held {
            flags |= flag;
        }
    }
    flags
}
