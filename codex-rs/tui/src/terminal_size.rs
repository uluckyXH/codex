//! Recover the missing terminal geometry in an HDC interactive shell.
//!
//! Some HarmonyOS emulator shells report a successful 0x0 window-size query and
//! reject TIOCSWINSZ. The explicit HDC launcher forwards the host dimensions via
//! LINES/COLUMNS. These hints affect layout only; valid native geometry wins.

pub(crate) fn resolve(size: (u16, u16)) -> (u16, u16) {
    #[cfg(target_env = "ohos")]
    {
        if size.0 > 0 && size.1 > 0 {
            return size;
        }
        let columns = std::env::var("COLUMNS").ok();
        let lines = std::env::var("LINES").ok();
        let recovered = resolve_ohos_size(size, columns.as_deref(), lines.as_deref());
        static WARNING: std::sync::Once = std::sync::Once::new();
        WARNING.call_once(|| {
            tracing::warn!(
                native_columns = size.0,
                native_rows = size.1,
                columns = recovered.0,
                rows = recovered.1,
                "HarmonyOS terminal reported an empty size; using valid LINES/COLUMNS or 80x24"
            );
        });
        recovered
    }
    #[cfg(not(target_env = "ohos"))]
    size
}

#[cfg(any(target_env = "ohos", test))]
fn resolve_ohos_size(native: (u16, u16), columns: Option<&str>, lines: Option<&str>) -> (u16, u16) {
    if native.0 > 0 && native.1 > 0 {
        return native;
    }
    let hint = columns
        .and_then(parse_dimension)
        .zip(lines.and_then(parse_dimension))
        .filter(|(width, height)| u32::from(*width) * u32::from(*height) <= 100_000);
    hint.unwrap_or((80, 24))
}

#[cfg(any(target_env = "ohos", test))]
fn parse_dimension(value: &str) -> Option<u16> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|value| (1..=1000).contains(value))
}

#[cfg(test)]
mod tests {
    use super::resolve_ohos_size;

    #[test]
    fn valid_native_geometry_takes_precedence_over_launch_hints() {
        assert_eq!(
            resolve_ohos_size((120, 40), Some("80"), Some("24")),
            (120, 40)
        );
        assert_eq!(
            resolve_ohos_size((2000, 2000), Some("80"), Some("24")),
            (2000, 2000)
        );
    }

    #[test]
    fn empty_native_geometry_uses_valid_launch_hints() {
        for size in [(0, 0), (80, 0), (0, 24)] {
            assert_eq!(resolve_ohos_size(size, Some("120"), Some("36")), (120, 36));
        }
    }

    #[test]
    fn both_dimensions_are_required() {
        for hints in [(None, None), (Some("120"), None), (None, Some("36"))] {
            assert_eq!(resolve_ohos_size((0, 0), hints.0, hints.1), (80, 24));
        }
    }

    #[test]
    fn invalid_hints_do_not_create_empty_or_unbounded_buffers() {
        for invalid in [
            "", "0", "1001", "65536", "-1", "+80", "80 ", " 80", "8x", "８０",
        ] {
            assert_eq!(
                resolve_ohos_size((0, 0), Some(invalid), Some("24")),
                (80, 24)
            );
            assert_eq!(
                resolve_ohos_size((0, 0), Some("80"), Some(invalid)),
                (80, 24)
            );
        }
        assert_eq!(
            resolve_ohos_size((0, 0), Some("1000"), Some("1000")),
            (80, 24)
        );
        assert_eq!(
            resolve_ohos_size((0, 0), Some("1000"), Some("100")),
            (1000, 100)
        );
    }

    #[cfg(not(target_env = "ohos"))]
    #[test]
    fn other_platforms_keep_native_geometry_including_zero() {
        for size in [(0, 0), (0, 24), (80, 0), (120, 36)] {
            assert_eq!(super::resolve(size), size);
        }
    }
}
