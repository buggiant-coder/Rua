#[cfg(target_os = "macos")]
fn main() {
    use objc2::{sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
    use objc2_foundation::{NSString, NSUserDefaults};
    use rua_desktop::text_input::{clean_edit_menu, configure};

    let mtm = MainThreadMarker::new().expect("AppKit main thread");
    configure();
    let defaults = NSUserDefaults::standardUserDefaults();
    for key in [
        "NSAutomaticCapitalizationEnabled",
        "WebAutomaticSpellingCorrectionEnabled",
        "WebAutomaticQuoteSubstitutionEnabled",
    ] {
        assert!(!defaults.boolForKey(&NSString::from_str(key)), "{key}");
    }
    let app = NSApplication::sharedApplication(mtm);
    let actions = [
        sel!(undo:),
        sel!(redo:),
        sel!(cut:),
        sel!(copy:),
        sel!(paste:),
        sel!(selectAll:),
    ];
    // Also exercise menu replacement when the Console language changes.
    for title in ["Edit", "编辑"] {
        let main = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Main"));
        let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        let parent = NSMenuItem::new(mtm);
        parent.setSubmenu(Some(&edit));
        main.addItem(&parent);
        for (index, action) in actions.iter().enumerate() {
            if index == 2 {
                edit.addItem(&NSMenuItem::separatorItem(mtm));
            }
            // Standard responder-chain editing actions; never invoked by this test.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(&format!("Action {index}")),
                    Some(*action),
                    &NSString::from_str(""),
                )
            };
            edit.addItem(&item);
        }
        app.setMainMenu(Some(&main));
        // System additions differ by macOS version; include a localized injected
        // submenu to prove cleanup does not depend on its displayed title.
        edit.addItem(&NSMenuItem::separatorItem(mtm));
        let injected = NSMenuItem::new(mtm);
        injected.setTitle(&NSString::from_str("自动填充"));
        injected.setSubmenu(Some(&NSMenu::new(mtm)));
        edit.addItem(&injected);
        assert!(edit.numberOfItems() > 7);
        clean_edit_menu(mtm, title);
        clean_edit_menu(mtm, title);
        assert_eq!(edit.numberOfItems(), 7);
        for (index, action) in actions.iter().enumerate() {
            let position = index + usize::from(index >= 2);
            assert_eq!(
                edit.itemAtIndex(position as isize).unwrap().action(),
                Some(*action)
            );
        }
        println!("PASS: {title} retains six editing actions and one separator");
    }
    println!("PASS: Rua text assistance is disabled");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
