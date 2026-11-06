use mac_health_monitor::config::{config_path, load_or_create};
use mac_health_monitor::monitor::Monitor;
use mac_health_monitor::ui;

fn main() {
    ui::app::run(Monitor::new(load_or_create(&config_path())));
}
