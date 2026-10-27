//! The capture options, their validation, and the browser's launch arguments.

use wg_types::WgError;

/// What a screenshot captures and how long it waits. The defaults are the Python ones.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureOptions {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    /// A CSS selector that must match before the capture.
    pub wait_for: Option<String>,
    /// A CSS selector that must stop matching before the capture.
    pub wait_gone: Option<String>,
    /// Seconds to pause after the page is ready, for layout and animations.
    pub settle: f64,
    /// Seconds allowed for the page to become ready.
    pub timeout: f64,
    /// `light` or `dark`: the emulated colour scheme.
    pub theme: Option<String>,
    /// Emulate a touch phone (mobile viewport and touch input).
    pub mobile: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            width: 1600,
            height: 1000,
            scale: 1.0,
            wait_for: None,
            wait_gone: Some("[aria-busy=\"true\"]".to_owned()),
            settle: 1.0,
            timeout: 60.0,
            theme: None,
            mobile: false,
        }
    }
}

impl CaptureOptions {
    /// Checks the sizes, scale and theme before any browser is started.
    ///
    /// # Errors
    /// [`ErrorCode::ValidationFailed`] for a viewport under 200 pixels, a scale outside
    /// 0.25 to 4, or a theme other than `light` or `dark`.
    pub fn validate(&self) -> Result<(), WgError> {
        if self.width < 200 || self.height < 200 || !(0.25..=4.0).contains(&self.scale) {
            return Err(WgError::validation(
                "width and height must be at least 200 and scale between 0.25 and 4",
            ));
        }
        if !matches!(self.theme.as_deref(), None | Some("light" | "dark")) {
            return Err(WgError::validation("theme must be light or dark"));
        }
        Ok(())
    }
}

/// The arguments that start a headless browser on `profile` with a window of `width` by
/// `height`, listening for DevTools on a port it picks itself.
#[must_use]
pub fn launch_args(profile: &str, width: u32, height: u32) -> Vec<String> {
    vec![
        "--headless=new".to_owned(),
        "--remote-debugging-port=0".to_owned(),
        format!("--user-data-dir={profile}"),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--disable-extensions".to_owned(),
        "--hide-scrollbars".to_owned(),
        format!("--window-size={width},{height}"),
        "about:blank".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_types::ErrorCode;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(CaptureOptions::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_small_viewports_and_bad_scale() {
        let small = CaptureOptions {
            width: 199,
            ..CaptureOptions::default()
        };
        assert!(small.validate().is_err());
        let scale = CaptureOptions {
            scale: f64::NAN,
            ..CaptureOptions::default()
        };
        assert!(scale.validate().is_err());
    }

    #[test]
    fn rejects_an_unknown_theme_with_its_message() {
        let sepia = CaptureOptions {
            theme: Some("sepia".to_owned()),
            ..CaptureOptions::default()
        };
        let err = sepia.validate().expect_err("sepia is not a theme");
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert!(err.message.contains("theme"));
    }

    #[test]
    fn launch_args_put_the_window_and_profile_in_place() {
        let args = launch_args("/tmp/p", 800, 600);
        assert!(args.contains(&"--user-data-dir=/tmp/p".to_owned()));
        assert!(args.contains(&"--window-size=800,600".to_owned()));
        assert_eq!(args.last().map(String::as_str), Some("about:blank"));
    }
}
