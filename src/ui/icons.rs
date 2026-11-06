//! Tinted SF Symbol images for the status item and the menu row indicators.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFontWeightRegular, NSImage, NSImageSymbolConfiguration};
use objc2_foundation::{NSArray, NSString};

use crate::menu_model::Indicator;
use crate::status::{IconKind, Status};

const STATUS_POINT_SIZE: f64 = 15.0;
const DOT_POINT_SIZE: f64 = 10.0;
const FALLBACK_SYMBOL: &str = "exclamationmark.triangle.fill";

fn symbol_name(icon: IconKind) -> &'static str {
    match icon {
        IconKind::ThumbsUp => "hand.thumbsup.fill",
        IconKind::Chip => "memorychip",
        IconKind::Drive => "internaldrive",
        IconKind::Recycle => "arrow.3.trianglepath",
        IconKind::Triangle => FALLBACK_SYMBOL,
    }
}

fn status_color(status: Status) -> Retained<NSColor> {
    match status {
        Status::Ok => NSColor::systemGreenColor(),
        Status::Warning => NSColor::systemYellowColor(),
        Status::Critical => NSColor::systemRedColor(),
    }
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Ok => "OK",
        Status::Warning => "warning",
        Status::Critical => "critical",
    }
}

/// Accessibility text for the menu-bar icon, e.g. "Health: swap critical".
fn status_description(icon: IconKind, status: Status) -> String {
    let subject = match icon {
        IconKind::ThumbsUp => return "Health: all OK".to_string(),
        IconKind::Chip => "memory",
        IconKind::Drive => "disk",
        IconKind::Recycle => "swap",
        IconKind::Triangle => "multiple issues",
    };
    format!("Health: {subject} {}", status_word(status))
}

/// Loads `name` (falling back to the triangle symbol), sizes it and tints it with `color`.
/// Never panics: if every lookup fails, an empty image is returned.
fn tinted_symbol(
    name: &str,
    description: &str,
    point_size: f64,
    color: &NSColor,
) -> Retained<NSImage> {
    let desc = NSString::from_str(description);
    let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&desc),
    )
    .or_else(|| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(FALLBACK_SYMBOL),
            Some(&desc),
        )
    });
    let Some(base) = base else {
        return NSImage::new();
    };
    // SAFETY: NSFontWeightRegular is an immutable AppKit constant.
    let weight = unsafe { NSFontWeightRegular };
    let palette =
        NSImageSymbolConfiguration::configurationWithPaletteColors(&NSArray::from_slice(&[color]));
    // Monochrome keeps multi-layer symbols' cutouts (the "!" in the triangle) transparent;
    // a one-color palette alone paints every layer and the triangle becomes a solid blob.
    let monochrome = NSImageSymbolConfiguration::configurationPreferringMonochrome();
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, weight)
        .configurationByApplyingConfiguration(&palette)
        .configurationByApplyingConfiguration(&monochrome);
    let image = base.imageWithSymbolConfiguration(&config).unwrap_or(base);
    image.setTemplate(false);
    image
}

/// Colored menu-bar icon (about 15 pt) for the given icon kind and status.
pub fn status_image(icon: IconKind, status: Status) -> Retained<NSImage> {
    tinted_symbol(
        symbol_name(icon),
        &status_description(icon, status),
        STATUS_POINT_SIZE,
        &status_color(status),
    )
}

/// Small colored dot (about 10 pt) shown left of a menu row. `None` for `Indicator::None`.
pub fn dot_image(ind: Indicator) -> Option<Retained<NSImage>> {
    let (color, description) = match ind {
        Indicator::Status(s) => (status_color(s), status_word(s)),
        Indicator::Info => (NSColor::secondaryLabelColor(), "info"),
        Indicator::None => return None,
    };
    Some(tinted_symbol(
        "circle.fill",
        description,
        DOT_POINT_SIZE,
        &color,
    ))
}
