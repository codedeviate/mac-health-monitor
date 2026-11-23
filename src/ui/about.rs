//! The "About Mac Health Monitor" panel: the standard AppKit about panel with the version and
//! a short description of the app.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSAboutPanelOptionVersion, NSApp, NSColor, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSLinkAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextAlignment,
};
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSDictionary, NSMutableAttributedString, NSRange,
    NSString, NSURL,
};

pub const APP_NAME: &str = "Mac Health Monitor";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/codedeviate/mac-health-monitor";

/// The description shown under the version, before the link line.
pub const BRIEF: &str = "Keeps an eye on disk space, memory, swap and memory pressure from the \
menu bar, and names the apps using the most memory. After a crash, or while the Mac is \
struggling, Health report… explains what happened in plain language.";

/// The license line and the repository link under the brief.
fn link_text() -> String {
    format!("MIT License\n{}", REPO_URL.trim_start_matches("https://"))
}

/// The panel's credits text: the brief, a blank line, then the license and link lines.
pub fn credits_text() -> String {
    format!("{BRIEF}\n\n{}", link_text())
}

/// Credits as centered small text in the label color (readable in light and dark mode), with
/// the repository part of the link line linking to `REPO_URL`.
fn credits() -> Retained<NSAttributedString> {
    let text = credits_text();
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    let font = NSFont::systemFontOfSize(11.0);
    let color = NSColor::labelColor();
    // SAFETY: the AppKit attribute-name statics are immutable constants.
    let keys = unsafe {
        [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let values: [&AnyObject; 3] = [font.as_ref(), color.as_ref(), style.as_ref()];
    let attrs = NSDictionary::from_slices(&keys, &values);
    // SAFETY: NSFont, NSColor and NSParagraphStyle are the value types AppKit expects for
    // these keys.
    let credits = unsafe {
        NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(&text),
            Some(&attrs),
        )
    };
    let repo = REPO_URL.trim_start_matches("https://");
    if let (Some(start), Some(url)) = (
        text.rfind(repo),
        NSURL::URLWithString(&NSString::from_str(REPO_URL)),
    ) {
        // NSRange counts UTF-16 code units; the brief contains a non-ASCII "…".
        let loc = text[..start].encode_utf16().count();
        let len = repo.encode_utf16().count();
        // SAFETY: NSURL is a valid NSLinkAttributeName value; the range lies inside the string.
        unsafe {
            credits.addAttribute_value_range(NSLinkAttributeName, &url, NSRange::new(loc, len))
        };
    }
    credits.into_super()
}

/// Brings the app forward and shows the about panel. The app is a menu-bar accessory, so
/// without activating it the panel would open behind the frontmost app.
pub fn show(mtm: MainThreadMarker) {
    let app = NSApp(mtm);
    let name = NSString::from_str(APP_NAME);
    let version = NSString::from_str(VERSION);
    // The build number would repeat the version in parentheses; leave it empty.
    let build = NSString::from_str("");
    let credits = credits();
    // SAFETY: the about-panel option keys are immutable constants.
    let keys = unsafe {
        [
            NSAboutPanelOptionApplicationName,
            NSAboutPanelOptionApplicationVersion,
            NSAboutPanelOptionVersion,
            NSAboutPanelOptionCredits,
        ]
    };
    let values: [&AnyObject; 4] = [
        name.as_ref(),
        version.as_ref(),
        build.as_ref(),
        credits.as_ref(),
    ];
    let options = NSDictionary::from_slices(&keys, &values);
    #[allow(deprecated)] // `activate` needs macOS 14; the app supports 13.
    app.activateIgnoringOtherApps(true);
    // SAFETY: the values are NSStrings and an NSAttributedString, as AppKit documents.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn credits_end_with_license_and_repository() {
        let text = credits_text();
        assert!(text.starts_with(BRIEF));
        assert!(text.ends_with("\n\nMIT License\ngithub.com/codedeviate/mac-health-monitor"));
    }
}
