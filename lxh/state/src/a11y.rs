//! Accessibility environment advertisement.
//!
//! Chromium and Electron only render their accessibility trees when the
//! session's `org.a11y.Bus` status marks accessibility as enabled. The
//! daemon flips that flag once at startup so applications launched on
//! harness displays expose AT-SPI trees. A screen reader the user already
//! runs (e.g. Orca) stays authoritative: `SCREEN_READER_ENABLED` is only
//! written when it is currently false.

use lxh_core::LxhError;

const ACCESSIBILITY_BUS_SERVICE: &str = "org.a11y.Bus";
const ACCESSIBILITY_BUS_OBJECT: &str = "/org/a11y/bus";
const ACCESSIBILITY_STATUS_INTERFACE: &str = "org.a11y.Status";
const ACCESSIBILITY_IS_ENABLED_PROPERTY: &str = "IsEnabled";
const SCREEN_READER_ENABLED_PROPERTY: &str = "ScreenReaderEnabled";

static ADVERTISED: std::sync::Once = std::sync::Once::new();

/// Advertise accessibility on the session bus exactly once per process.
/// Errors are surfaced to the caller the first time; later calls are no-ops.
pub fn advertise_once() -> Result<(), LxhError> {
    let mut result = Ok(());
    ADVERTISED.call_once(|| {
        result = advertise();
    });
    result
}

fn advertise() -> Result<(), LxhError> {
    // zbus blocking calls must not run on a tokio worker thread; use a
    // dedicated OS thread with its own connection.
    std::thread::Builder::new()
        .name("lxh-a11y-advertise".into())
        .spawn(advertise_blocking)
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
        .join()
        .map_err(|_| LxhError::ProcessSpawnFailed("a11y advertise thread panicked".into()))?
}

fn advertise_blocking() -> Result<(), LxhError> {
    let session = zbus::blocking::Connection::session()
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    let status = zbus::blocking::Proxy::new(
        &session,
        ACCESSIBILITY_BUS_SERVICE,
        ACCESSIBILITY_BUS_OBJECT,
        ACCESSIBILITY_STATUS_INTERFACE,
    )
    .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;

    // Respect a screen reader the user already runs: only write when the
    // flag is currently false, so an active Orca session stays authoritative
    // and we avoid emitting a redundant PropertiesChanged.
    let screen_reader: bool = status
        .get_property(SCREEN_READER_ENABLED_PROPERTY)
        .unwrap_or(false);
    if !screen_reader {
        status
            .set_property(SCREEN_READER_ENABLED_PROPERTY, true)
            .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    }
    let enabled: bool = status
        .get_property(ACCESSIBILITY_IS_ENABLED_PROPERTY)
        .unwrap_or(false);
    if !enabled {
        status
            .set_property(ACCESSIBILITY_IS_ENABLED_PROPERTY, true)
            .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?;
    }
    Ok(())
}
