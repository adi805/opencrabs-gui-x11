//! The GUI theme catalog: what the picker offers and what it guarantees.
//!
//! The window used to draw four hardcoded palettes. It now serves the TUI's
//! 31-theme pack, projected onto the twenty GUI fields by
//! `scripts/gen_gui_palettes.py`, plus anything a user drops in the profile's
//! `gui_themes/` folder.
//!
//! Two things have to stay true, or the feature is a regression dressed as a
//! feature: every shipped palette must be readable (a theme whose body text
//! disappears is not a theme), and the loader must degrade instead of dying
//! (a hand-edited file cannot take the window down).

use crate::config::gui_palette::{self, CatalogEntry};
use crate::config::gui_theme::{self, LIGHT, ORGN, Palette};

/// Body text on its own canvas. Below this, a card is a rectangle that says
/// nothing.
const TEXT_FLOOR: f64 = 4.5;

/// The dim tiers: hints and metadata, deliberately quiet. The floor is what the
/// TUI pack itself guarantees, so a GUI palette is never quieter than the
/// terminal theme it came from.
const DIM_FLOOR: f64 = 2.0;

fn catalog() -> Vec<CatalogEntry> {
    gui_palette::catalog()
}

#[test]
fn the_whole_pack_is_offered() {
    let rows = catalog();
    assert!(
        rows.len() >= 31,
        "expected the full pack, got {} themes",
        rows.len()
    );
    let mut slugs: Vec<&str> = rows.iter().map(|row| row.slug.as_str()).collect();
    slugs.sort_unstable();
    let unique = slugs.len();
    slugs.dedup();
    assert_eq!(unique, slugs.len(), "two cards share a slug: {slugs:?}");
    for must in ["nord", "gruvbox", "tokyonight", "catppuccin-frappe"] {
        assert!(
            slugs.contains(&must),
            "'{must}' is missing from the catalog: {slugs:?}"
        );
    }
}

/// Every palette the picker can hand the window is readable. This is the
/// certificate for the projection: a role mapped to the wrong source key shows
/// up here as text that vanished into its background.
#[test]
fn every_offered_palette_is_readable() {
    let mut worst = (f64::MAX, String::new());
    for row in catalog() {
        let palette = row.palette;
        let text = gui_theme::contrast_ratio(palette.text, palette.bg);
        assert!(
            text >= TEXT_FLOOR,
            "{}: body text is {text:.2}:1 on its background",
            row.slug
        );
        for (role, colour) in [
            ("text_normal", palette.text_normal),
            ("text_muted", palette.text_muted),
            ("text_faint", palette.text_faint),
        ] {
            let ratio = gui_theme::contrast_ratio(colour, palette.bg);
            assert!(
                ratio >= DIM_FLOOR,
                "{}: {role} is {ratio:.2}:1 on its background",
                row.slug
            );
            if ratio < worst.0 {
                worst = (ratio, format!("{} {role}", row.slug));
            }
        }
        // The loader lifts an accent that does not read, so no palette can
        // ship a ring nobody can see.
        let accent = gui_theme::contrast_ratio(palette.accent, palette.bg);
        assert!(
            accent >= gui_theme::MIN_ACCENT_CONTRAST,
            "{}: accent is {accent:.2}:1, the loader should have lifted it",
            row.slug
        );
    }
    assert!(
        worst.0 >= DIM_FLOOR,
        "the dimmest tier is {} at {:.2}:1",
        worst.1,
        worst.0
    );
}

/// The palette is the pack's, not a reshuffle of `orgn`: a projection that
/// silently fell back to the base would pass the readability test and look
/// identical on every card.
#[test]
fn a_catalog_palette_is_not_the_orgn_base() {
    let nord = gui_palette::lookup("nord").expect("nord ships with the binary");
    assert_ne!(nord, ORGN, "nord resolved to the default palette");
    // `#2e3440` is nord's background in the pack; the projection must carry it.
    assert_eq!(nord.bg, 0x2e3440, "nord's canvas came from the wrong role");
}

/// A file with only some fields set inherits the rest, so a user can change one
/// colour without retyping twenty.
#[test]
fn a_partial_file_inherits_the_base() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("half.toml"),
        "bg = \"#101820\"\nname = \"Half\"\n",
    )
    .unwrap();
    let mut into = std::collections::BTreeMap::new();
    gui_palette::read_into(dir.path(), &mut into);
    let entry = into.get("half").expect("the file is a palette");
    assert_eq!(entry.palette.bg, 0x101820);
    assert_eq!(entry.palette.panel, ORGN.panel, "panel was not inherited");
    assert_eq!(entry.label, "Half", "the display name comes from the file");
}

/// A user file with a built-in's name replaces it: that is how someone retunes
/// `nord` without forking the binary.
#[test]
fn a_user_file_overrides_a_built_in_of_the_same_name() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("nord.toml"),
        "bg = \"#000010\"\npanel = \"#000020\"\n",
    )
    .unwrap();
    let mut into = std::collections::BTreeMap::new();
    gui_palette::read_into(dir.path(), &mut into);
    let entry = into.get("nord").expect("nord is still a palette");
    assert_eq!(entry.palette.bg, 0x000010, "the user's canvas did not win");
    assert_eq!(
        entry.label, "nord",
        "a file without `name` is labelled by stem"
    );
}

/// A file that is not a palette is skipped, not fatal: an empty file, a broken
/// TOML, and a wrong extension all leave the catalog standing.
#[test]
fn junk_in_the_folder_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.toml"), "").unwrap();
    std::fs::write(dir.path().join("broken.toml"), "bg = \n[oops").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "bg = \"#ffffff\"").unwrap();
    std::fs::write(dir.path().join("real.toml"), "bg = \"#123456\"\n").unwrap();
    let mut into = std::collections::BTreeMap::new();
    gui_palette::read_into(dir.path(), &mut into);
    let slugs: Vec<&str> = into.keys().map(|k| k.as_str()).collect();
    assert_eq!(
        slugs,
        ["real"],
        "only the readable palette survives: {slugs:?}"
    );
}

/// A missing folder is the normal first run, not an error.
#[test]
fn a_missing_folder_yields_no_entries() {
    let mut into = std::collections::BTreeMap::new();
    gui_palette::read_into(std::path::Path::new("/nonexistent/gui_themes"), &mut into);
    assert!(into.is_empty());
}

/// A named theme replaces the mode's base, and an unknown name falls back to
/// it, so a deleted theme does not leave the window unthemed.
#[test]
fn resolve_palette_prefers_the_named_theme_and_falls_back() {
    let nord = gui_palette::lookup("nord").expect("nord ships with the binary");
    assert_eq!(
        gui_theme::resolve_palette(crate::config::GuiTheme::Light, false, None, Some("nord")),
        nord,
        "a named theme should win over the mode's base"
    );
    assert_eq!(
        gui_theme::resolve_palette(
            crate::config::GuiTheme::Light,
            false,
            None,
            Some("no-such-theme")
        ),
        LIGHT,
        "an unknown name falls back to the mode's own palette"
    );
    assert_eq!(
        gui_theme::resolve_palette(crate::config::GuiTheme::Orgn, true, None, None),
        ORGN,
        "no name at all is the built-in mode"
    );
}

/// `custom` still paints its accent on a catalog palette, which is what keeps
/// the colour wheel useful after the catalog lands.
#[test]
fn custom_still_paints_its_accent_over_a_catalog_theme() {
    let nord = gui_palette::lookup("nord").expect("nord ships with the binary");
    let picked = gui_theme::resolve_palette(
        crate::config::GuiTheme::Custom,
        true,
        Some("#ff0000"),
        Some("nord"),
    );
    assert_ne!(picked.accent, nord.accent, "the wheel's accent was ignored");
    assert_eq!(picked.bg, nord.bg, "the catalog theme's canvas was lost");
}

/// The generated files and the pack source cannot drift: the script that wrote
/// them says so, and this is the test that runs it.
#[test]
fn the_generated_palettes_match_the_pack_source() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/gen_gui_palettes.py");
    if !script.exists() {
        // A packaged crate has no `scripts/`; nothing to compare against.
        return;
    }
    let out = std::process::Command::new("python3")
        .arg(&script)
        .arg("--check")
        .output();
    match out {
        Ok(out) => assert!(
            out.status.success(),
            "generated palettes are stale: {}",
            String::from_utf8_lossy(&out.stdout)
        ),
        // No python on this machine: the check is advisory, not a failure.
        Err(_) => eprintln!("python3 not found; skipping the generated-palette check"),
    }
}

/// The catalog is what the modal draws, so its palette type has to be the one
/// the window paints with: a stray `Palette` alias would compile and then
/// disagree at runtime.
#[test]
fn catalog_palettes_are_window_palettes() {
    let rows = catalog();
    let first: Palette = rows[0].palette;
    assert_eq!(first, rows[0].palette);
}
