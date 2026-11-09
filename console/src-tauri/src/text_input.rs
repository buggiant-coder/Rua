use objc2::{sel, MainThreadMarker};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSString, NSUserDefaults};

/// Set app-local preferences before AppKit/WebKit cache text-checking defaults.
pub fn configure() {
    let defaults = NSUserDefaults::standardUserDefaults();
    for key in [
        "NSAutomaticCapitalizationEnabled",
        "NSAutomaticSpellingCorrectionEnabled",
        "NSAutomaticQuoteSubstitutionEnabled",
        "NSAutomaticDashSubstitutionEnabled",
        "NSAutomaticPeriodSubstitutionEnabled",
        "NSAutomaticTextReplacementEnabled",
        "NSAutomaticTextCompletionEnabled",
        "WebAutomaticSpellingCorrectionEnabled",
        "WebAutomaticQuoteSubstitutionEnabled",
        "WebAutomaticDashSubstitutionEnabled",
        "WebAutomaticTextReplacementEnabled",
        "WebContinuousSpellCheckingEnabled",
    ] {
        defaults.setBool_forKey(false, &NSString::from_str(key));
    }
}

/// AppKit appends AutoFill, Dictation and Emoji when installing the main menu.
/// Keep only Rua's editing actions, independent of the system language.
pub fn clean_edit_menu(mtm: MainThreadMarker, title: &str) {
    let Some(menu) = NSApplication::sharedApplication(mtm).mainMenu() else {
        return;
    };
    for index in 0..menu.numberOfItems() {
        let Some(edit) = menu.itemAtIndex(index).and_then(|item| item.submenu()) else {
            continue;
        };
        if edit.title().to_string() != title {
            continue;
        }
        let actions = [
            sel!(undo:),
            sel!(redo:),
            sel!(cut:),
            sel!(copy:),
            sel!(paste:),
            sel!(selectAll:),
        ];
        for index in (0..edit.numberOfItems()).rev() {
            let Some(item) = edit.itemAtIndex(index) else {
                continue;
            };
            if !item.isSeparatorItem()
                && !item
                    .action()
                    .is_some_and(|action| actions.contains(&action))
            {
                edit.removeItemAtIndex(index);
            }
        }
        while edit.numberOfItems() > 0
            && edit
                .itemAtIndex(edit.numberOfItems() - 1)
                .is_some_and(|item| item.isSeparatorItem())
        {
            edit.removeItemAtIndex(edit.numberOfItems() - 1);
        }
    }
}
