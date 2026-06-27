//! Adwaita desktop GUI for SystemPrune.

mod window;

use adw::prelude::*;
use gtk::{glib, gio};

const APP_ID: &str = "io.github.systemprune.Gui";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(window::build_window);
    app.run_with_args::<&str>(&[])
}

pub fn run() -> glib::ExitCode {
    main()
}
