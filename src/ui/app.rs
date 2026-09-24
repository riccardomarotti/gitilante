//! Application bootstrap and global shortcuts (SPEC section 30).

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::git::repository::Repository;
use crate::ui::main_window::MainWindow;

/// Application id used for the GApplication registration.
pub const APP_ID: &str = "dev.gitilante.Gitilante";

/// Compact badge styling for the History refs (BRANCH.md sections 41-42).
const BADGE_CSS: &str = r#"
.ref-badge {
    padding: 1px 6px;
    border-radius: 6px;
    background-color: alpha(@window_fg_color, 0.08);
    font-size: 0.85em;
}
.ref-remote {
    background-color: alpha(@accent_bg_color, 0.12);
}
.ref-current,
.ref-head {
    background-color: alpha(@accent_bg_color, 0.22);
    font-weight: bold;
}
"#;

/// Runs the Gitilante GUI on `repo`, returning the process exit code.
pub fn run(repo: Repository) -> i32 {
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.set_accels_for_action("win.refresh", &["<Primary>r"]);

    // Ref badge styling for the History graph (BRANCH.md section 41).
    app.connect_startup(|_| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(BADGE_CSS);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    app.set_accels_for_action("app.quit", &["<Primary>q"]);
    // Contextual hunk shortcuts: they act on the focused hunk (SPEC §30).
    app.set_accels_for_action("win.stage-hunk", &["s"]);
    app.set_accels_for_action("win.unstage-hunk", &["u"]);
    app.set_accels_for_action("win.discard-hunk", &["d"]);
    app.set_accels_for_action("win.revert-hunk", &["r"]);
    let quit = gio::SimpleAction::new("quit", None);
    {
        let app = app.clone();
        quit.connect_activate(move |_, _| app.quit());
    }
    app.add_action(&quit);

    // The controller must outlive the callbacks; keep one per application.
    let window_slot: Rc<RefCell<Option<MainWindow>>> = Rc::new(RefCell::new(None));
    {
        let slot = window_slot.clone();
        app.connect_activate(move |app| {
            let mut slot = slot.borrow_mut();
            let window = slot.get_or_insert_with(|| MainWindow::new(app, repo.clone()));
            window.present();
        });
    }

    app.run_with_args(&["gitilante"]).into()
}
