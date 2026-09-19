//! macOS capture backend.
//!
//! - Displays and window z-order come from Core Graphics.
//! - The frontmost application is the owner of the frontmost normal window.
//! - Pixels come from ScreenCaptureKit (`SCScreenshotManager`, macOS 14+).
//!
//! ScreenCaptureKit is asynchronous; its completion handlers run on an
//! internal queue and hand results back through a channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use argus_protocol::{Application, Bounds, Frame, FrameId, PixelBuffer, Timestamp};
use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDirectDisplayID, CGDisplayBounds,
    CGDisplayCopyDisplayMode, CGDisplayMode, CGError, CGGetActiveDisplayList, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRectMakeWithDictionaryRepresentation, CGRequestScreenCaptureAccess,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName,
    kCGWindowOwnerPID,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
    SCStreamErrorCode,
};

use crate::{
    CaptureBackend, CaptureTarget, DisplayId, DisplayInfo, Error, Result, WindowId, WindowInfo,
};

/// How long to wait for ScreenCaptureKit before giving up.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on connected displays.
const MAX_DISPLAYS: usize = 32;

/// Source of process-unique frame identifiers.
static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);

/// Capture backend for macOS.
#[derive(Debug)]
pub struct MacCaptureBackend {
    _private: (),
}

impl MacCaptureBackend {
    /// Creates the backend.
    pub fn new() -> Self {
        // A process without NSApplication has no WindowServer connection
        // until the first Core Graphics display call; ScreenCaptureKit window
        // filters abort (`CGS_REQUIRE_INIT`) without one.
        let _ = CGMainDisplayID();
        Self { _private: () }
    }
}

impl Default for MacCaptureBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBackend for MacCaptureBackend {
    fn has_permission(&self) -> bool {
        CGPreflightScreenCaptureAccess()
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        let mut ids: [CGDirectDisplayID; MAX_DISPLAYS] = [0; MAX_DISPLAYS];
        let mut count = 0u32;
        // SAFETY: `ids` has room for `MAX_DISPLAYS` entries and `count` is a
        // valid out-pointer.
        let status =
            unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
        if status != CGError::Success {
            return Err(Error::Platform(format!("CGGetActiveDisplayList failed: {status:?}")));
        }

        let main = CGMainDisplayID();
        ids[..count as usize]
            .iter()
            .map(|&id| {
                Ok(DisplayInfo {
                    id,
                    bounds: bounds_from_rect(CGDisplayBounds(id))?,
                    scale_factor: display_scale_factor(id),
                    is_main: id == main,
                })
            })
            .collect()
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let options =
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
        let list = CGWindowListCopyWindowInfo(options, 0)
            .ok_or_else(|| Error::Platform("CGWindowListCopyWindowInfo failed".to_owned()))?;
        // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries
        // with string keys.
        let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };

        let mut applications = HashMap::new();
        let mut windows = Vec::with_capacity(list.len());
        for info in list.iter() {
            // SAFETY: the `kCGWindow*` keys are immutable constants.
            let (number_key, pid_key, layer_key, bounds_key, name_key, owner_key, alpha_key) = unsafe {
                (
                    kCGWindowNumber,
                    kCGWindowOwnerPID,
                    kCGWindowLayer,
                    kCGWindowBounds,
                    kCGWindowName,
                    kCGWindowOwnerName,
                    kCGWindowAlpha,
                )
            };
            // Fully transparent windows are invisible helpers.
            if dict_f64(&info, alpha_key).is_some_and(|alpha| alpha <= 0.0) {
                continue;
            }
            let (Some(id), Some(pid), Some(layer), Some(bounds)) = (
                dict_i64(&info, number_key),
                dict_i64(&info, pid_key),
                dict_i64(&info, layer_key),
                dict_rect(&info, bounds_key),
            ) else {
                continue;
            };
            let (Ok(id), Ok(pid)) = (WindowId::try_from(id), u32::try_from(pid)) else {
                continue;
            };

            let application = applications
                .entry(pid)
                .or_insert_with(|| application_for_pid(pid, dict_string(&info, owner_key)))
                .clone();
            windows.push(WindowInfo {
                id,
                title: dict_string(&info, name_key).filter(|title| !title.is_empty()),
                application,
                bounds: bounds_from_rect(bounds)?,
                layer,
            });
        }
        Ok(windows)
    }

    /// The owner of the frontmost normal window.
    ///
    /// `NSWorkspace.frontmostApplication` is not used: without a running main
    /// loop (a CLI, a daemon) it keeps reporting whatever was frontmost when
    /// the process started — e.g. `loginwindow` after the screen was locked.
    /// The window list is always current.
    fn frontmost_application(&self) -> Result<Option<Application>> {
        Ok(self.windows()?.into_iter().find(|window| window.layer == 0).map(|w| w.application))
    }

    fn capture(&self, target: CaptureTarget) -> Result<Frame> {
        ensure_permission()?;
        let content = shareable_content()?;

        let (filter, bounds) = match target {
            CaptureTarget::MainDisplay => display_filter(&content, CGMainDisplayID())?,
            CaptureTarget::Display(id) => display_filter(&content, id)?,
            CaptureTarget::Window(id) => window_filter(&content, id)?,
            CaptureTarget::FrontmostWindow => window_filter(&content, self.frontmost_window()?.id)?,
        };

        // SAFETY: plain property read on a valid filter.
        let scale_factor = unsafe { filter.pointPixelScale() };
        let config = screenshot_config(bounds, scale_factor);
        let image = screenshot(&filter, &config)?;
        tracing::debug!(
            ?target,
            width = image.pixels.width(),
            height = image.pixels.height(),
            "captured frame"
        );

        let id = FrameId(NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed));
        Ok(Frame::new(id, image.timestamp, bounds_from_rect(bounds)?, scale_factor, image.pixels)?)
    }
}

/// Checks the Screen Recording permission, asking the system to prompt the
/// user when it has not been granted yet.
fn ensure_permission() -> Result<()> {
    if CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() {
        Ok(())
    } else {
        Err(Error::PermissionDenied)
    }
}

fn display_scale_factor(id: DisplayId) -> f32 {
    CGDisplayCopyDisplayMode(id)
        .map(|mode| {
            let pixels = CGDisplayMode::pixel_width(Some(&mode)) as f32;
            let points = CGDisplayMode::width(Some(&mode)) as f32;
            pixels / points
        })
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0)
}

fn bounds_from_rect(rect: CGRect) -> Result<Bounds> {
    Ok(Bounds::new(
        rect.origin.x as f32,
        rect.origin.y as f32,
        rect.size.width as f32,
        rect.size.height as f32,
    )?)
}

fn dict_i64(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

fn dict_f64(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<f64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_f64()
}

fn dict_string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    Some(dict.get(key)?.downcast::<CFString>().ok()?.to_string())
}

fn dict_rect(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CGRect> {
    let value = dict.get(key)?.downcast::<CFDictionary>().ok()?;
    let mut rect = CGRect::default();
    // SAFETY: `value` is a valid dictionary and `rect` a valid out-pointer.
    unsafe { CGRectMakeWithDictionaryRepresentation(Some(&value), &mut rect) }.then_some(rect)
}

fn application_for_pid(pid: u32, fallback_name: Option<String>) -> Application {
    let running = i32::try_from(pid)
        .ok()
        .and_then(NSRunningApplication::runningApplicationWithProcessIdentifier);
    match running {
        Some(app) => {
            let mut application = application_from(&app);
            application.name = application.name.or(fallback_name);
            // The window list's owner is authoritative: for some processes
            // (Tk applications such as IDLE) NSRunningApplication reports
            // no process identifier.
            application.pid = Some(pid);
            application
        }
        None => Application { name: fallback_name, bundle_id: None, pid: Some(pid) },
    }
}

fn application_from(app: &NSRunningApplication) -> Application {
    Application {
        name: app.localizedName().map(|name| name.to_string()),
        bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
        pid: u32::try_from(app.processIdentifier()).ok(),
    }
}

/// Converts an `NSError` delivered by ScreenCaptureKit.
fn sc_error(error: *mut NSError, what: &str) -> Error {
    // SAFETY: ScreenCaptureKit passes either null or a valid error that lives
    // for the duration of the completion handler.
    let Some(error) = (unsafe { error.as_ref() }) else {
        return Error::Platform(format!("{what} failed without an error"));
    };
    if error.code() == SCStreamErrorCode::UserDeclined.0 {
        return Error::PermissionDenied;
    }
    Error::Platform(format!(
        "{what} failed: {} (code {})",
        error.localizedDescription(),
        error.code()
    ))
}

fn receive<T>(receiver: &mpsc::Receiver<Result<T>>, what: &'static str) -> Result<T> {
    receiver.recv_timeout(TIMEOUT).map_err(|_| Error::Timeout(what))?
}

/// Fetches the capturable displays and on-screen windows.
fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: `content` is null or a valid object; retaining it keeps it
        // alive after the handler returns. SCShareableContent is immutable.
        let result = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(content),
            None => Err(sc_error(error, "listing shareable content")),
        };
        let _ = sender.send(result);
    });
    // SAFETY: the handler matches the expected block signature and is kept
    // alive by ScreenCaptureKit until it has been called.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &handler,
        );
    }
    receive(&receiver, "shareable content")
}

fn display_filter(
    content: &SCShareableContent,
    id: DisplayId,
) -> Result<(Retained<SCContentFilter>, CGRect)> {
    // SAFETY: plain property reads on valid ScreenCaptureKit objects.
    let display = unsafe { content.displays() }
        .iter()
        .find(|display| unsafe { display.displayID() } == id)
        .ok_or(Error::DisplayNotFound(id))?;
    // SAFETY: `display` is a valid SCDisplay and the exclusion list is empty.
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &NSArray::new(),
        )
    };
    // SAFETY: plain property read.
    Ok((filter, unsafe { display.frame() }))
}

fn window_filter(
    content: &SCShareableContent,
    id: WindowId,
) -> Result<(Retained<SCContentFilter>, CGRect)> {
    // SAFETY: plain property reads on valid ScreenCaptureKit objects.
    let window = unsafe { content.windows() }
        .iter()
        .find(|window| unsafe { window.windowID() } == id)
        .ok_or(Error::WindowNotFound(id))?;
    // SAFETY: `window` is a valid SCWindow.
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
    };
    // SAFETY: plain property read.
    Ok((filter, unsafe { window.frame() }))
}

fn screenshot_config(bounds: CGRect, scale_factor: f32) -> Retained<SCStreamConfiguration> {
    let scale = f64::from(scale_factor);
    // SAFETY: creating and configuring a fresh configuration object.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth((bounds.size.width * scale).round() as usize);
        config.setHeight((bounds.size.height * scale).round() as usize);
        config.setShowsCursor(false);
        // Keep the image aligned with the window frame.
        config.setIgnoreShadowsSingleWindow(true);
        config
    }
}

struct CapturedImage {
    timestamp: Timestamp,
    pixels: PixelBuffer,
}

fn screenshot(filter: &SCContentFilter, config: &SCStreamConfiguration) -> Result<CapturedImage> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let timestamp = Timestamp::now();
        // SAFETY: `image` is null or a valid image for the duration of the
        // handler; it is fully copied before returning.
        let result = match unsafe { image.as_ref() } {
            Some(image) => rgba_pixels(image).map(|pixels| CapturedImage { timestamp, pixels }),
            None => Err(sc_error(error, "screenshot")),
        };
        let _ = sender.send(result);
    });
    // SAFETY: filter and configuration are valid; the handler matches the
    // expected block signature.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            filter,
            config,
            Some(&handler),
        );
    }
    receive(&receiver, "screenshot")
}

/// Copies an image into a tightly packed sRGB RGBA buffer.
fn rgba_pixels(image: &CGImage) -> Result<PixelBuffer> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    let bytes_per_row = width
        .checked_mul(PixelBuffer::BYTES_PER_PIXEL)
        .ok_or_else(|| Error::Platform("image too large".to_owned()))?;
    let mut data = vec![0u8; bytes_per_row * height];

    // SAFETY: `kCGColorSpaceSRGB` is an immutable constant.
    let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| Error::Platform("sRGB color space unavailable".to_owned()))?;
    let bitmap_info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY: `data` holds `bytes_per_row * height` bytes and outlives the
    // context, which is dropped at the end of this function.
    let context = unsafe {
        CGBitmapContextCreate(
            data.as_mut_ptr().cast(),
            width,
            height,
            8,
            bytes_per_row,
            Some(&color_space),
            bitmap_info,
        )
    }
    .ok_or_else(|| Error::Platform("cannot create bitmap context".to_owned()))?;

    let rect = CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&context), rect, Some(image));
    drop(context);

    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(Error::Platform("image too large".to_owned()));
    };
    Ok(PixelBuffer::new(width, height, data)?)
}
