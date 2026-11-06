//! Application delegate: owns the status item, the menu, the monitor and the refresh timer.

use std::cell::{OnceCell, RefCell};
use std::time::Instant;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSTimer,
};

use super::{icons, menu};
use crate::config::{config_path, load_or_create};
use crate::monitor::Monitor;
use crate::{metrics, processes};

pub struct Ivars {
    monitor: RefCell<Monitor>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    menu: Retained<NSMenu>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `AppDelegate` does not implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct AppDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: NSApplicationDelegate has no safety requirements.
    unsafe impl NSApplicationDelegate for AppDelegate {
        // SAFETY: the signature matches `applicationDidFinishLaunching:`.
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
            item.setMenu(Some(&self.ivars().menu));
            let _ = self.ivars().status_item.set(item);
            self.refresh();
            self.schedule_timer();
        }
    }

    impl AppDelegate {
        // SAFETY: the signature matches an NSTimer action (`- (void)tick:(NSTimer *)timer`).
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.refresh();
        }

        // SAFETY: the signature matches a menu item action (`- (void)openConfig:(id)sender`).
        #[unsafe(method(openConfig:))]
        fn open_config(&self, _sender: Option<&AnyObject>) {
            match std::process::Command::new("open").arg("-t").arg(config_path()).status() {
                Ok(s) if s.success() => {}
                Ok(s) => eprintln!("open config: `open -t` exited with {s}"),
                Err(e) => eprintln!("open config: {e}"),
            }
        }

        // SAFETY: the signature matches a menu item action (`- (void)reloadConfig:(id)sender`).
        #[unsafe(method(reloadConfig:))]
        fn reload_config(&self, _sender: Option<&AnyObject>) {
            let changed = {
                let mut monitor = self.ivars().monitor.borrow_mut();
                let before = monitor.config().refresh_secs;
                monitor.set_config(load_or_create(&config_path()));
                monitor.config().refresh_secs != before
            };
            if changed {
                self.schedule_timer();
            }
            self.refresh();
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker, monitor: Monitor) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            monitor: RefCell::new(monitor),
            status_item: OnceCell::new(),
            menu: NSMenu::new(mtm),
            timer: RefCell::new(None),
        });
        // SAFETY: NSObject's `init` has this signature.
        unsafe { msg_send![super(this), init] }
    }

    /// Samples, evaluates and updates the icon and menu. On a sampling error the UI is left as is.
    fn refresh(&self) {
        autoreleasepool(|_| {
            let snap = match metrics::sample() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("sample failed: {e}");
                    return;
                }
            };
            let tick =
                self.ivars()
                    .monitor
                    .borrow_mut()
                    .tick(&snap, Instant::now(), processes::sample);
            if let Some(button) = self
                .ivars()
                .status_item
                .get()
                .and_then(|i| i.button(self.mtm()))
            {
                button.setImage(Some(&icons::status_image(tick.icon, tick.status)));
            }
            let target: &AnyObject = self.as_ref();
            menu::render(&self.ivars().menu, &tick.menu, target);
        });
    }

    /// (Re)creates the refresh timer from the current `refresh_secs`, in the common run-loop
    /// modes so the menu keeps updating while it is open.
    fn schedule_timer(&self) {
        if let Some(old) = self.ivars().timer.borrow_mut().take() {
            old.invalidate();
        }
        let secs = self.ivars().monitor.borrow().config().refresh_secs as f64;
        // SAFETY: `self` implements `tick:` with the NSTimer action signature.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                secs,
                self.as_ref(),
                sel!(tick:),
                None,
                true,
            )
        };
        // SAFETY: NSRunLoopCommonModes is an immutable Foundation constant; the timer is valid.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        *self.ivars().timer.borrow_mut() = Some(timer);
    }
}

/// Runs the menu-bar app on the main thread. Never returns.
pub fn run(monitor: Monitor) -> ! {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("mac-health-monitor must run on the main thread");
        std::process::exit(1);
    };
    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm, monitor);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    std::process::exit(0);
}
