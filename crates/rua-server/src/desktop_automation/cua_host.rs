//! Pinned Cua SDK host lifecycle and permission probes, without its in-process capture runtime.
//! Upstream source hashes and the extraction boundary are recorded in automation/manifest.json.
//! MIT; see ../automation/cua-driver-LICENSE.

mod embedded;
pub use embedded::*;

/// The TCC grants required by a macOS host application before it starts an
/// embedded driver. These probes execute in the importing SDK process so the
/// operating system attributes the request to the host application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacOsPermissionStatus {
    pub accessibility: bool,
    pub screen_recording: bool,
}

pub fn current_mac_os_permission_status() -> MacOsPermissionStatus {
    #[cfg(target_os = "macos")]
    {
        MacOsPermissionStatus {
            accessibility: mac_os_accessibility_granted(),
            screen_recording: mac_os_screen_recording_granted(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        MacOsPermissionStatus {
            accessibility: true,
            screen_recording: true,
        }
    }
}

pub fn request_mac_os_permissions() -> MacOsPermissionStatus {
    #[cfg(target_os = "macos")]
    {
        MacOsPermissionStatus {
            accessibility: request_mac_os_accessibility(),
            screen_recording: request_mac_os_screen_recording(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        MacOsPermissionStatus {
            accessibility: true,
            screen_recording: true,
        }
    }
}

#[cfg(target_os = "macos")]
fn mac_os_accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(target_os = "macos")]
fn request_mac_os_accessibility() -> bool {
    use core_foundation::base::TCFType as _;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::CFString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

#[cfg(target_os = "macos")]
fn mac_os_screen_recording_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(target_os = "macos")]
fn request_mac_os_screen_recording() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}
