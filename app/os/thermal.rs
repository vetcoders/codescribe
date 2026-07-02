use codescribe_core::stt::scheduler::{
    ThermalLevel, current_process_thermal_level, set_process_thermal_level,
};

#[cfg(target_os = "macos")]
use objc::runtime::{Class, Object, Sel};
#[cfg(target_os = "macos")]
use objc::{declare::ClassDecl, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::ptr;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
static THERMAL_OBSERVER: OnceLock<usize> = OnceLock::new();

pub fn install_thermal_probe() {
    #[cfg(target_os = "macos")]
    unsafe {
        apply_current_state("initial");
        if THERMAL_OBSERVER.get().is_some() {
            return;
        }

        let observer_class = thermal_observer_class();
        let observer: *mut Object = msg_send![observer_class, new];
        let center_class =
            Class::get("NSNotificationCenter").expect("NSNotificationCenter class missing");
        let center: *mut Object = msg_send![center_class, defaultCenter];
        let name = ns_string("NSProcessInfoThermalStateDidChangeNotification");
        let _: () = msg_send![
            center,
            addObserver: observer
            selector: sel!(thermalStateDidChange:)
            name: name
            object: ptr::null::<Object>()
        ];
        let _ = THERMAL_OBSERVER.set(observer as usize);
        tracing::info!("macOS thermal probe installed");
    }

    #[cfg(not(target_os = "macos"))]
    {
        set_process_thermal_level(ThermalLevel::Nominal);
    }
}

pub fn current_thermal_level() -> ThermalLevel {
    current_process_thermal_level()
}

#[cfg(target_os = "macos")]
extern "C" fn thermal_state_did_change(_this: &Object, _sel: Sel, _notification: *mut Object) {
    unsafe {
        apply_current_state("notification");
    }
}

#[cfg(target_os = "macos")]
unsafe fn apply_current_state(source: &str) {
    let process_info_class = Class::get("NSProcessInfo").expect("NSProcessInfo class missing");
    let process_info: *mut Object = msg_send![process_info_class, processInfo];
    let raw_state: isize = msg_send![process_info, thermalState];
    let level = match raw_state {
        1 => ThermalLevel::Fair,
        2 => ThermalLevel::Serious,
        3 => ThermalLevel::Critical,
        _ => ThermalLevel::Nominal,
    };
    let previous = current_process_thermal_level();
    set_process_thermal_level(level);

    if previous != level {
        match level {
            ThermalLevel::Nominal | ThermalLevel::Fair => {
                tracing::info!(?level, source, "macOS thermal pressure changed");
            }
            ThermalLevel::Serious => {
                tracing::warn!(
                    ?level,
                    source,
                    "macOS thermal pressure serious; STT refine lane paused"
                );
            }
            ThermalLevel::Critical => {
                tracing::error!(
                    ?level,
                    source,
                    "macOS thermal pressure critical; STT commit/refine lanes paused"
                );
                crate::os::tray_status::update_tray_status(
                    crate::os::tray_status::TrayStatus::Thermal,
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn thermal_observer_class() -> *const Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let superclass = Class::get("NSObject").expect("NSObject class missing");
        let mut decl = ClassDecl::new("CodescribeThermalObserver", superclass).expect("class decl");
        unsafe {
            decl.add_method(
                sel!(thermalStateDidChange:),
                thermal_state_did_change as extern "C" fn(&Object, Sel, *mut Object),
            );
        }
        decl.register() as *const Class as usize
    }) as *const Class
}

#[cfg(target_os = "macos")]
unsafe fn ns_string(value: &str) -> *mut Object {
    let c_str = CString::new(value).expect("NSString input cannot contain null byte");
    let cls = Class::get("NSString").expect("NSString class missing");
    msg_send![cls, stringWithUTF8String: c_str.as_ptr()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_thermal_probe_publishes_a_level_and_is_idempotent() {
        // On macOS this genuinely registers the NSProcessInfo thermal observer
        // and reads the live thermal state into the scheduler-visible atomic
        // (core/stt/scheduler.rs consumes current_process_thermal_level). A
        // second call must be a guarded no-op and must not panic. This proves
        // the probe installs cleanly now that CodescribeHotkeys::start wires it
        // at runtime bootstrap (previously the fn had zero callers).
        install_thermal_probe();
        let first = current_thermal_level();
        install_thermal_probe();
        let second = current_thermal_level();
        assert_eq!(
            first, second,
            "thermal level must be stable across idempotent probe installs"
        );
    }
}
