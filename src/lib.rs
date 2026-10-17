//! Device Battery for Windows: a desktop widget and tray icon that show the battery of
//! wireless mice, keyboards and headphones, including 2.4 GHz receivers Windows cannot read.

pub mod common;

#[cfg(windows)]
pub mod win;

#[cfg(test)]
mod packaging {
    /// `rc.exe` reads a resource script in the system code page, so a stray non-ASCII character
    /// (an arrow in a comment, say) depends on the build machine's locale.
    #[test]
    fn the_resource_script_is_ascii() {
        let script = include_str!("../assets/app.rc");
        assert!(
            script.is_ascii(),
            "assets/app.rc must stay ASCII: rc.exe reads it in the ANSI code page"
        );
    }

    /// `assets/app.rc` spells the version out by hand, so a version bump in Cargo.toml would
    /// otherwise leave stale file properties on the exe. Check every field that carries it.
    #[test]
    fn the_resource_version_matches_the_crate_version() {
        let script = include_str!("../assets/app.rc");
        let version = env!("CARGO_PKG_VERSION");
        let core = version.split(['-', '+']).next().unwrap_or(version);
        let mut parts: Vec<u16> = core
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        parts.resize(3, 0);
        let numeric = format!("{},{},{},0", parts[0], parts[1], parts[2]);
        for expected in [
            format!("FILEVERSION {numeric}"),
            format!("PRODUCTVERSION {numeric}"),
            format!("VALUE \"FileVersion\", \"{version}\""),
            format!("VALUE \"ProductVersion\", \"{version}\""),
        ] {
            assert!(
                script.contains(&expected),
                "assets/app.rc must contain `{expected}` (Cargo.toml says {version})"
            );
        }
    }
}
