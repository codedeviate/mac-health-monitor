//! Renders a `MenuModel` into an `NSMenu`.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSApp, NSAttributedStringAttachmentConveniences, NSFont, NSFontAttributeName, NSMenu,
    NSMenuItem, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextAlignment,
    NSTextAttachment, NSTextTab,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSMutableCopying, NSRange, NSString, ns_string,
};

use super::icons::dot_image;
use crate::menu_model::{MenuModel, Row};

/// Location of the right-aligned tab stop that lines up the values.
const VALUE_TAB_LOCATION: f64 = 300.0;

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &NSString,
) -> Retained<NSMenuItem> {
    // SAFETY: `action` is either None or a selector implemented by the item's target.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            key,
        )
    }
}

fn disabled_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let it = item(mtm, title, None, ns_string!(""));
    it.setEnabled(false);
    it
}

/// "[dot]  {label}\t{value}" with the value right-aligned at `VALUE_TAB_LOCATION`.
///
/// The status dot is a text attachment rather than `NSMenuItem.image`: recent macOS releases
/// do not draw item images in this menu at all, and an attachment also keeps its full color
/// on disabled (display-only) rows, where an item image would be dimmed.
fn aligned_title(row: &Row) -> Retained<NSAttributedString> {
    // SAFETY: an empty options dictionary is valid for NSTextTab.
    let tab = unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Right,
            VALUE_TAB_LOCATION,
            &NSDictionary::new(),
        )
    };
    let style = NSMutableParagraphStyle::new();
    style.setTabStops(Some(&NSArray::from_retained_slice(&[tab])));
    let font = NSFont::menuFontOfSize(0.0);
    // SAFETY: the AppKit attribute-name statics are immutable constants.
    let keys = unsafe { [NSFontAttributeName, NSParagraphStyleAttributeName] };
    let values: [&AnyObject; 2] = [font.as_ref(), style.as_ref()];
    let attrs = NSDictionary::from_slices(&keys, &values);
    let dot = dot_image(row.indicator);
    let prefix = if dot.is_some() { "  " } else { "" };
    let text = NSString::from_str(&format!("{prefix}{}\t{}", row.label, row.value));
    // SAFETY: NSFont for the font key and NSParagraphStyle for the paragraph style key are
    // the value types AppKit expects.
    let body = unsafe { NSAttributedString::new_with_attributes(&text, &attrs) };
    let Some(dot) = dot else {
        return body;
    };
    let attachment = NSTextAttachment::new();
    attachment.setImage(Some(&dot));
    let title = NSAttributedString::attributedStringWithAttachment(&attachment).mutableCopy();
    title.appendAttributedString(&body);
    // The paragraph style (tab stop) must cover the whole string, attachment included.
    // SAFETY: as above; the range spans the string just built.
    unsafe {
        title.addAttribute_value_range(
            NSParagraphStyleAttributeName,
            &style,
            NSRange::new(0, title.length()),
        )
    };
    title.into_super()
}

fn row_item(mtm: MainThreadMarker, row: &Row) -> Retained<NSMenuItem> {
    let it = disabled_item(mtm, "");
    it.setAttributedTitle(Some(&aligned_title(row)));
    it
}

/// Replaces the contents of `menu` with `model`. The config items target `target`,
/// which must implement `openConfig:` and `reloadConfig:`.
pub fn render(menu: &NSMenu, model: &MenuModel, target: &AnyObject) {
    let mtm = menu.mtm();
    menu.setAutoenablesItems(false);
    menu.removeAllItems();

    if let Some(msg) = &model.config_error {
        menu.addItem(&disabled_item(mtm, &format!("⚠ config error: {msg}")));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    for row in &model.params {
        menu.addItem(&row_item(mtm, row));
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&disabled_item(mtm, "Potential culprits"));
    for row in &model.culprits {
        let it = row_item(mtm, row);
        it.setIndentationLevel(1);
        menu.addItem(&it);
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let open = item(mtm, "Open config…", Some(sel!(openConfig:)), ns_string!(""));
    let reload = item(
        mtm,
        "Reload config",
        Some(sel!(reloadConfig:)),
        ns_string!(""),
    );
    let quit = item(mtm, "Quit", Some(sel!(terminate:)), ns_string!("q"));
    let app = NSApp(mtm);
    // SAFETY: `target` implements openConfig:/reloadConfig: (caller contract) and
    // NSApplication implements terminate:. NSMenuItem targets are weak; both outlive the menu.
    unsafe {
        open.setTarget(Some(target));
        reload.setTarget(Some(target));
        quit.setTarget(Some(&app));
    }
    menu.addItem(&open);
    menu.addItem(&reload);
    menu.addItem(&quit);
}
