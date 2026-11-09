use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSControlStateValueOn};
use objc2_foundation::NSString;

use crate::preferences::DesktopLabels;

/// Runs on the AppKit thread, independently of Console or Runtime availability.
pub(crate) fn show(labels: DesktopLabels) -> Result<Option<bool>, String> {
    let mtm = MainThreadMarker::new().ok_or("quit dialog requires the main thread")?;
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(labels.quit_confirmation));
    alert.setInformativeText(&NSString::from_str(labels.quit_message));
    alert.addButtonWithTitle(&NSString::from_str(labels.quit));
    let cancel = alert.addButtonWithTitle(&NSString::from_str(labels.cancel));
    cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
    alert.setShowsSuppressionButton(true);
    let checkbox = alert
        .suppressionButton()
        .ok_or("quit dialog checkbox unavailable")?;
    checkbox.setTitle(&NSString::from_str(labels.confirm_before_quit));
    checkbox.setState(NSControlStateValueOn);

    if alert.runModal() == NSAlertFirstButtonReturn {
        Ok(Some(checkbox.state() == NSControlStateValueOn))
    } else {
        Ok(None)
    }
}
