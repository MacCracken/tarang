//! Shared VA-API utilities for encoder and decoder modules.

use crate::core::{Result, TarangError};
use cros_libva::Display;
use std::path::Path;
use std::rc::Rc;

/// Open a VA-API display, optionally at a specific device path.
///
/// If `device` is `None`, tries render nodes 128–135 in order.
pub fn open_display(device: &Option<String>) -> Result<Rc<Display>> {
    if let Some(path) = device {
        Display::open_drm_display(Path::new(path))
            .map_err(|e| TarangError::HwAccelError(format!("failed to open {path}: {e:?}").into()))
    } else {
        for i in 128..136 {
            let path = format!("/dev/dri/renderD{i}");
            if let Ok(display) = Display::open_drm_display(Path::new(&path)) {
                return Ok(display);
            }
        }
        Err(TarangError::HwAccelError(
            "no VA-API render node found".into(),
        ))
    }
}

/// Map a VA-API error into a `TarangError::HwAccelError` with context.
pub fn va_err<T, E: std::fmt::Debug>(result: std::result::Result<T, E>, msg: &str) -> Result<T> {
    result.map_err(|e| TarangError::HwAccelError(format!("{msg}: {e:?}").into()))
}
