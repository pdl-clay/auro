// SPDX-License-Identifier: MIT OR Apache-2.0

//! Startup checks for the display backend.
//!
//! GPUI 0.2.2 assumes a desktop compositor: it unwraps `wl_seat` and binds fixed version ranges for
//! `wl_compositor`, `wl_shm` and `xdg_wm_base`. A minimal or headless compositor aborts the process
//! before the app can report anything, so the required globals are checked before GPUI starts.

use auro_core::AuroError;

/// Interfaces GPUI binds unconditionally, with the minimum advertised version it accepts.
///
/// GPUI binds `wl_compositor` for `wl_surface` versions 3 through 6, `xdg_wm_base` for 2 through
/// 5, and `wl_shm` version 1.
#[cfg(target_os = "linux")]
const REQUIRED_GLOBALS: &[(&str, u32)] = &[
    ("wl_compositor", 3),
    ("wl_shm", 1),
    ("xdg_wm_base", 2),
    ("wl_seat", 5),
];

/// Multi-instance globals GPUI inspects per advertised object; every one must meet the version.
#[cfg(target_os = "linux")]
const PER_INSTANCE_GLOBALS: &[(&str, u32)] = &[("wl_output", 2)];

/// Fails with [`AuroError::Platform`] when the selected display cannot host a GPUI window.
#[cfg(target_os = "linux")]
pub(crate) fn preflight() -> Result<(), AuroError> {
    preflight_wayland()
}

/// Non-Linux backends do not need a display-global check: GPUI's platform constructor returns or
/// reports its own error there.
#[cfg(not(target_os = "linux"))]
pub(crate) fn preflight() -> Result<(), AuroError> {
    Ok(())
}

/// Checks the globals GPUI 0.2.2 requires when it starts on Wayland.
///
/// GPUI picks Wayland whenever `WAYLAND_DISPLAY` is set and non-empty, and picks its headless
/// client when `ZED_HEADLESS` is set; this follows the same selection so it never rejects a display
/// GPUI would not use.
#[cfg(target_os = "linux")]
fn preflight_wayland() -> Result<(), AuroError> {
    use wayland_client::Connection;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::{Event as RegistryEvent, WlRegistry};
    use wayland_client::{Dispatch, QueueHandle};

    struct RegistryState;

    impl Dispatch<WlRegistry, GlobalListContents> for RegistryState {
        fn event(
            _state: &mut Self,
            _proxy: &WlRegistry,
            _event: RegistryEvent,
            _data: &GlobalListContents,
            _conn: &Connection,
            _qhandle: &QueueHandle<Self>,
        ) {
        }
    }

    if std::env::var_os("ZED_HEADLESS").is_some() {
        return Ok(());
    }
    let Some(display) = std::env::var_os("WAYLAND_DISPLAY") else {
        return Ok(());
    };
    if display.is_empty() {
        return Ok(());
    }
    let display = display.to_string_lossy();

    let connection = Connection::connect_to_env().map_err(|error| {
        AuroError::platform(format!(
            "cannot connect to Wayland display `{display}`: {error}"
        ))
    })?;
    let (globals, _queue) = registry_queue_init::<RegistryState>(&connection).map_err(|error| {
        AuroError::platform(format!(
            "cannot read the Wayland registry from `{display}`: {error}"
        ))
    })?;

    let mut advertised = Vec::new();
    globals.contents().with_list(|list| {
        for global in list {
            advertised.push((global.interface.clone(), global.version));
        }
    });

    match unsatisfied_requirement(&advertised) {
        Some(requirement) => Err(AuroError::platform(format!(
            "Wayland display `{display}` cannot host a GPUI window: {requirement}"
        ))),
        None => Ok(()),
    }
}

/// Returns the first missing or too-old global, if any.
#[cfg(target_os = "linux")]
fn unsatisfied_requirement(advertised: &[(String, u32)]) -> Option<String> {
    for (interface, minimum) in REQUIRED_GLOBALS {
        let Some((_, version)) = advertised.iter().find(|(name, _)| name == interface) else {
            return Some(format!(
                "the compositor does not provide `{interface}` (version {minimum} or newer)"
            ));
        };
        if version < minimum {
            return Some(format!(
                "the compositor provides `{interface}` version {version}, but version {minimum} or newer is required"
            ));
        }
    }

    for (name, version) in advertised {
        if let Some((_, minimum)) = PER_INSTANCE_GLOBALS
            .iter()
            .find(|(interface, _)| interface == name)
            && version < minimum
        {
            return Some(format!(
                "the compositor provides `{name}` version {version}, but version {minimum} or newer is required"
            ));
        }
    }

    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn globals(entries: &[(&str, u32)]) -> Vec<(String, u32)> {
        entries
            .iter()
            .map(|(name, version)| ((*name).to_owned(), *version))
            .collect()
    }

    #[test]
    fn accepts_the_minimum_versions() {
        let advertised = globals(&[
            ("wl_compositor", 3),
            ("wl_shm", 1),
            ("xdg_wm_base", 2),
            ("wl_seat", 5),
            ("wl_output", 2),
        ]);
        assert_eq!(unsatisfied_requirement(&advertised), None);
    }

    #[test]
    fn rejects_a_compositor_without_a_seat() {
        let advertised = globals(&[("wl_compositor", 6), ("wl_shm", 1), ("xdg_wm_base", 2)]);
        let requirement = unsatisfied_requirement(&advertised).expect("missing seat");
        assert!(requirement.contains("wl_seat"), "{requirement}");
    }

    #[test]
    fn rejects_a_too_old_global() {
        let advertised = globals(&[
            ("wl_compositor", 2),
            ("wl_shm", 1),
            ("xdg_wm_base", 2),
            ("wl_seat", 5),
        ]);
        let requirement = unsatisfied_requirement(&advertised).expect("old compositor");
        assert!(requirement.contains("wl_compositor"), "{requirement}");
        assert!(requirement.contains("version 2"), "{requirement}");
    }

    #[test]
    fn rejects_an_old_output_instance() {
        let advertised = globals(&[
            ("wl_compositor", 6),
            ("wl_shm", 1),
            ("xdg_wm_base", 2),
            ("wl_seat", 5),
            ("wl_output", 1),
        ]);
        let requirement = unsatisfied_requirement(&advertised).expect("old output");
        assert!(requirement.contains("wl_output"), "{requirement}");
    }

    #[test]
    fn ignores_optional_missing_globals() {
        let advertised = globals(&[
            ("wl_compositor", 6),
            ("wl_shm", 1),
            ("xdg_wm_base", 2),
            ("wl_seat", 5),
        ]);
        assert_eq!(unsatisfied_requirement(&advertised), None);
    }
}
