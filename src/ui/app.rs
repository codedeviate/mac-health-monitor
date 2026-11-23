//! Application delegate: owns the status item, the menu, the monitor and the refresh timer.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Instant;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSTimer,
};

use super::{about, icons, menu};
use crate::clock::{boot_time, now_unix};
use crate::config::{config_path, load_or_create};
use crate::history::{HistorySample, Recorder, history_path};
use crate::menu_model::{MenuModel, ReportState};
use crate::monitor::Monitor;
use crate::report::{self, Crash};
use crate::{metrics, processes};

/// How often the main thread checks whether a background report or crash check has finished.
const REPORT_POLL_SECS: f64 = 0.25;

type ReportResult = Result<PathBuf, String>;

pub struct Ivars {
    monitor: RefCell<Monitor>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    menu: Retained<NSMenu>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    recorder: RefCell<Recorder>,
    /// Unacknowledged crash behind the current boot; shown as the menu's first line.
    crash: RefCell<Option<Crash>>,
    /// Result channel and poll timer of the running crash check, if any.
    crash_rx: RefCell<Option<Receiver<Option<Crash>>>>,
    crash_timer: RefCell<Option<Retained<NSTimer>>>,
    /// When the last crash check was started.
    crash_checked_at: Cell<Option<i64>>,
    report: RefCell<ReportState>,
    /// Result channel and poll timer of the report being generated, if any.
    report_rx: RefCell<Option<Receiver<ReportResult>>>,
    report_timer: RefCell<Option<Retained<NSTimer>>>,
    /// Crash shown when the running report started; only that one is acknowledged by it, since
    /// a crash found later by a re-check may not be in that report.
    report_crash: Cell<Option<Crash>>,
    /// Menu from the latest refresh, so report-state changes can re-render without resampling.
    last_menu: RefCell<Option<MenuModel>>,
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
            // The first refresh also starts the crash check in the background.
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

        // SAFETY: the signature matches an NSTimer action (`- (void)reportPoll:(NSTimer *)timer`).
        #[unsafe(method(reportPoll:))]
        fn report_poll(&self, _timer: &NSTimer) {
            self.poll_report();
        }

        // SAFETY: the signature matches an NSTimer action (`- (void)crashPoll:(NSTimer *)timer`).
        #[unsafe(method(crashPoll:))]
        fn crash_poll(&self, _timer: &NSTimer) {
            self.poll_crash();
        }

        // SAFETY: the signature matches a menu item action (`- (void)healthReport:(id)sender`).
        #[unsafe(method(healthReport:))]
        fn health_report(&self, _sender: Option<&AnyObject>) {
            self.start_report();
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

        // SAFETY: the signature matches a menu item action (`- (void)about:(id)sender`).
        #[unsafe(method(about:))]
        fn about(&self, _sender: Option<&AnyObject>) {
            about::show(self.mtm());
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
            recorder: RefCell::new(Recorder::new(history_path())),
            crash: RefCell::new(None),
            crash_rx: RefCell::new(None),
            crash_timer: RefCell::new(None),
            crash_checked_at: Cell::new(None),
            report: RefCell::new(ReportState::Idle),
            report_rx: RefCell::new(None),
            report_timer: RefCell::new(None),
            report_crash: Cell::new(None),
            last_menu: RefCell::new(None),
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
            let now = now_unix();
            let history_cfg = self.ivars().monitor.borrow().config().history.clone();
            let observed = self.ivars().recorder.borrow_mut().observe(
                now,
                || HistorySample::from_tick(now, &snap, &tick.eval, &tick.culprits),
                &history_cfg,
            );
            if let Err(e) = observed {
                eprintln!("history: {e}");
            }
            *self.ivars().last_menu.borrow_mut() = Some(tick.menu);
            self.render_menu();
        });
        self.maybe_check_crash();
    }

    /// Starts crash detection on a background thread when it is due (see
    /// `report::should_check_crash`).
    fn maybe_check_crash(&self) {
        let now = now_unix();
        let known = self.ivars().crash.borrow().is_some();
        let running = self.ivars().crash_rx.borrow().is_some();
        let last = self.ivars().crash_checked_at.get();
        if !report::should_check_crash(now, boot_time(), last, known, running) {
            return;
        }
        self.ivars().crash_checked_at.set(Some(now));
        let config = self.ivars().monitor.borrow().config().clone();
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("crash-check".into())
            .spawn(move || {
                // The receiver may be gone if the app is quitting; nothing to do then.
                let _ = tx.send(report::startup_crash(&config));
            });
        if let Err(e) = spawned {
            eprintln!("crash check: could not start: {e}");
            return;
        }
        *self.ivars().crash_rx.borrow_mut() = Some(rx);
        let timer = self.poll_timer(sel!(crashPoll:));
        if let Some(old) = self.ivars().crash_timer.borrow_mut().replace(timer) {
            old.invalidate();
        }
    }

    /// Main-thread completion of a background crash check: shows the crash line if one was
    /// found. Does nothing until the result has arrived.
    fn poll_crash(&self) {
        let found = {
            let rx = self.ivars().crash_rx.borrow();
            let Some(rx) = rx.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(c) => c,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => None,
            }
        };
        *self.ivars().crash_rx.borrow_mut() = None;
        if let Some(timer) = self.ivars().crash_timer.borrow_mut().take() {
            timer.invalidate();
        }
        if found.is_some() {
            *self.ivars().crash.borrow_mut() = found;
            self.render_menu();
        }
    }

    /// A repeating timer calling `action` on `self` every `REPORT_POLL_SECS`, added to the
    /// main run loop in the common modes so it also fires while the menu is open.
    fn poll_timer(&self, action: Sel) -> Retained<NSTimer> {
        // SAFETY: callers pass a selector `self` implements with the NSTimer action signature.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                REPORT_POLL_SECS,
                self.as_ref(),
                action,
                None,
                true,
            )
        };
        // SAFETY: NSRunLoopCommonModes is an immutable Foundation constant; the timer is valid.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        timer
    }

    /// Renders the latest menu model with the current crash line and report state.
    fn render_menu(&self) {
        let Some(mut model) = self.ivars().last_menu.borrow().clone() else {
            return;
        };
        let now = now_unix();
        let days = i64::try_from(self.ivars().monitor.borrow().config().report.incident_days)
            .unwrap_or(30);
        let mut crash = self.ivars().crash.borrow_mut();
        // The line goes away on its own once the crash leaves the report window.
        if crash.is_some_and(|c| c.time < now - days * 86_400) {
            *crash = None;
        }
        model.crash = crash.as_ref().map(|c| report::crash_line(c, now));
        drop(crash);
        model.report = self.ivars().report.borrow().clone();
        let target: &AnyObject = self.as_ref();
        menu::render(&self.ivars().menu, &model, target);
    }

    /// Starts generating a report on a background thread; ignored while one is running.
    fn start_report(&self) {
        if *self.ivars().report.borrow() == ReportState::Busy {
            return;
        }
        let config = self.ivars().monitor.borrow().config().clone();
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("health-report".into())
            .spawn(move || {
                // The receiver may be gone if the app is quitting; nothing to do then.
                let _ = tx.send(report::generate(&config));
            });
        if let Err(e) = spawned {
            *self.ivars().report.borrow_mut() =
                ReportState::Failed(format!("could not start: {e}"));
            self.render_menu();
            return;
        }
        *self.ivars().report.borrow_mut() = ReportState::Busy;
        *self.ivars().report_rx.borrow_mut() = Some(rx);
        self.ivars().report_crash.set(*self.ivars().crash.borrow());
        let timer = self.poll_timer(sel!(reportPoll:));
        if let Some(old) = self.ivars().report_timer.borrow_mut().replace(timer) {
            old.invalidate();
        }
        self.render_menu();
    }

    /// Main-thread completion of a background report: opens it, acknowledges a shown crash
    /// and updates the menu. Does nothing until the result has arrived.
    fn poll_report(&self) {
        let result = {
            let rx = self.ivars().report_rx.borrow();
            let Some(rx) = rx.as_ref() else {
                return;
            };
            match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("report thread stopped".to_string()),
            }
        };
        *self.ivars().report_rx.borrow_mut() = None;
        if let Some(timer) = self.ivars().report_timer.borrow_mut().take() {
            timer.invalidate();
        }
        let state = match result {
            Ok(path) => {
                open_report(path);
                let shown = self.ivars().report_crash.take();
                let mut crash = self.ivars().crash.borrow_mut();
                if let Some(c) = shown
                    && *crash == shown
                {
                    *crash = None;
                    drop(crash);
                    if let Err(e) = report::acknowledge(c.time) {
                        eprintln!("could not record crash acknowledgement: {e}");
                    }
                }
                ReportState::Idle
            }
            Err(e) => ReportState::Failed(e),
        };
        *self.ivars().report.borrow_mut() = state;
        self.render_menu();
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

/// Opens `path` in the user's app for `.md` files, falling back to the default text editor.
/// Waits for `open` on a helper thread so the main thread never blocks.
fn open_report(path: PathBuf) {
    let spawned = thread::Builder::new()
        .name("open-report".into())
        .spawn(move || {
            let ok = matches!(Command::new("open").arg(&path).status(), Ok(s) if s.success());
            if ok {
                return;
            }
            match Command::new("open").arg("-t").arg(&path).status() {
                Ok(s) if s.success() => {}
                Ok(s) => eprintln!("open report: `open -t` exited with {s}"),
                Err(e) => eprintln!("open report: {e}"),
            }
        });
    if let Err(e) = spawned {
        eprintln!("open report: could not start helper thread: {e}");
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
