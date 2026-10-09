//! The desktop's text size, followed as the interface's zoom.
//!
//! GNOME's `text-scaling-factor` (`org.gnome.desktop.interface`) is the
//! desktop-wide text size: GNOME's Large Text sets it, and so does Omarchy's
//! `omarchy display text size` (12 px is 1.0, 14 px about 1.18). The desktop
//! portal publishes it on Linux; elsewhere the interface keeps its own size.
//!
//! The whole interface zooms rather than only its fonts, so panels sized for
//! their labels grow with them.

/// The settings namespace holding the text size.
const NAMESPACE: &str = "org.gnome.desktop.interface";
/// The text size key in [`NAMESPACE`].
const KEY: &str = "text-scaling-factor";
/// The zooms followed; GNOME's own range for the key is 0.5 to 3.
const RANGE: std::ops::RangeInclusive<f64> = 0.5..=3.0;

/// The interface zoom for a `text-scaling-factor`, or None for a value that is
/// not a usable size.
fn zoom(factor: f64) -> Option<f32> {
    RANGE.contains(&factor).then_some(factor as f32)
}

/// Zooms the interface to the desktop's text size now and, on Linux, whenever
/// it changes.
///
/// Reading it is one portal call that gives up after about a second. Changes
/// arrive on a thread that blocks on the portal's signals for the life of the
/// process; it holds no state to save, so it is never joined and ends with the
/// process. Without a session bus or portal the interface stays at 1.0.
pub(crate) fn follow(ctx: &eframe::egui::Context) {
    #[cfg(target_os = "linux")]
    if let Err(error) = portal::follow(ctx) {
        eprintln!("RAWmakase: not following the desktop's text size: {error}");
    }
    #[cfg(not(target_os = "linux"))]
    let _ = ctx;
}

#[cfg(target_os = "linux")]
mod portal {
    use super::{KEY, NAMESPACE, zoom};
    use std::time::Duration;
    use zbus::blocking::{Connection, Proxy, connection};
    use zbus::zvariant::{OwnedValue, Value};

    pub(super) fn follow(ctx: &eframe::egui::Context) -> zbus::Result<()> {
        let connection = connection::Builder::session()?
            .method_timeout(Duration::from_secs(1))
            .build()?;
        let proxy = settings(&connection)?;
        if let Some(zoom) = read(&proxy) {
            ctx.set_zoom_factor(zoom);
        }
        let signals = proxy.receive_signal_with_args("SettingChanged", &[(0, NAMESPACE)])?;
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("text-scale".into())
            .spawn(move || {
                // The proxy owns the match rule; keep it alive with the iterator.
                let _proxy = proxy;
                for message in signals {
                    let Ok((_, key, value)) =
                        message.body().deserialize::<(String, String, OwnedValue)>()
                    else {
                        continue;
                    };
                    if key == KEY
                        && let Some(zoom) = number(&value).and_then(zoom)
                    {
                        ctx.set_zoom_factor(zoom);
                    }
                }
            })
            .map_err(|error| zbus::Error::Failure(error.to_string()))?;
        Ok(())
    }

    fn settings(connection: &Connection) -> zbus::Result<Proxy<'static>> {
        Proxy::new(
            connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )
    }

    /// Reads the key through `ReadOne` (portal version 2) or the older `Read`,
    /// which wraps the value in a second variant.
    fn read(proxy: &Proxy<'_>) -> Option<f32> {
        let args = (NAMESPACE, KEY);
        let value: OwnedValue = proxy
            .call("ReadOne", &args)
            .or_else(|_| proxy.call("Read", &args))
            .ok()?;
        number(&value).and_then(zoom)
    }

    pub(super) fn number(value: &Value<'_>) -> Option<f64> {
        match value {
            Value::F64(n) => Some(*n),
            Value::Value(inner) => number(inner),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_factor_is_the_zoom() {
        assert_eq!(zoom(1.0), Some(1.0));
        // `omarchy display text size 14` with an 11 pt interface font.
        assert_eq!(zoom(1.1818), Some(1.1818));
        assert_eq!(zoom(0.5), Some(0.5));
        assert_eq!(zoom(3.0), Some(3.0));
    }

    #[test]
    fn unusable_factors_are_ignored() {
        assert_eq!(zoom(0.0), None);
        assert_eq!(zoom(-1.0), None);
        assert_eq!(zoom(0.49), None);
        assert_eq!(zoom(3.5), None);
        assert_eq!(zoom(f64::NAN), None);
        assert_eq!(zoom(f64::INFINITY), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn values_unwrap_nested_variants() {
        use zbus::zvariant::Value;
        assert_eq!(portal::number(&Value::F64(1.25)), Some(1.25));
        let nested = Value::Value(Box::new(Value::F64(1.5)));
        assert_eq!(portal::number(&nested), Some(1.5));
        assert_eq!(portal::number(&Value::from("1.5")), None);
    }
}
