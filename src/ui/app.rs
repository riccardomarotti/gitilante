//! Application bootstrap and global shortcuts (SPEC section 30).

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::cli::InitialView;
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

/// The single-key hunk shortcuts (SPEC section 30) are suspended while a search
/// bar has the keyboard focus, so typing is never intercepted
/// (GITILANTE_SEARCH_SPEC.md section 51).
pub fn suspend_hunk_shortcuts(app: &adw::Application, suspended: bool) {
    let (stage, unstage, discard, revert): (&[&str], &[&str], &[&str], &[&str]) = if suspended {
        (&[], &[], &[], &[])
    } else {
        (&["s"], &["u"], &["d"], &["r"])
    };
    app.set_accels_for_action("win.stage-hunk", stage);
    app.set_accels_for_action("win.unstage-hunk", unstage);
    app.set_accels_for_action("win.discard-hunk", discard);
    app.set_accels_for_action("win.revert-hunk", revert);
}

/// Runs the Gitilante GUI on `repo`, showing `initial_view` after the first
/// refresh (GITILANTE_CLI_FILE_HISTORY_SPEC.md §12-§14).
pub fn run(repo: Repository, initial_view: InitialView) -> i32 {
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.set_accels_for_action("win.refresh", &["<Primary>r"]);
    app.set_accels_for_action("win.search", &["<Primary>f"]);
    app.set_accels_for_action(
        "win.global-search",
        &["<Primary><Shift>f", "<Primary><Shift>F"],
    );

    // Ref badge styling for the History graph (BRANCH.md section 41).
    app.connect_startup(|_| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(BADGE_CSS);
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
            let window = slot
                .get_or_insert_with(|| MainWindow::new(app, repo.clone(), initial_view.clone()));
            window.present();
        });
    }

    app.run_with_args(&["gitilante"]).into()
}
