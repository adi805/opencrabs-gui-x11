//! The window's theme catalog.
//!
//! The desktop window used to draw exactly four palettes (#2619), all compiled
//! into `gui_theme`. The terminal ships 31 themes; `scripts/gen_gui_palettes.py`
//! projects each of them onto the twenty fields of [`Palette`], and this module
//! serves that catalog to the window.
//!
//! Two layers, one name space: the built-ins are embedded in the binary so a
//! fresh install has the whole catalog with nothing on disk, and every
//! `*.toml` in the profile's `gui_themes/` folder is layered on top, replacing a
//! built-in of the same name or adding a new one. A file that does not parse is
//! skipped with a log line; it never takes the window down.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::Deserialize;

use crate::config::gui_palettes_builtin::BUILTIN;
use crate::config::gui_theme::{self, ORGN, Palette};

/// The folder a user drops palettes into, under the active profile's home.
pub const DIR_NAME: &str = "gui_themes";

/// One row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The file stem: what `[gui] palette` stores.
    pub slug: String,
    /// What the card is called on screen.
    pub label: String,
    /// The colours the card shows and the window adopts.
    pub palette: Palette,
}

/// A palette file: every field optional, so a file can override just the
/// background and inherit the rest.
#[derive(Debug, Default, Deserialize)]
struct PaletteFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    bg: Option<String>,
    #[serde(default)]
    panel: Option<String>,
    #[serde(default)]
    panel_hi: Option<String>,
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_strong: Option<String>,
    #[serde(default)]
    text_normal: Option<String>,
    #[serde(default)]
    text_muted: Option<String>,
    #[serde(default)]
    text_title: Option<String>,
    #[serde(default)]
    text_faint: Option<String>,
    #[serde(default)]
    text_hint: Option<String>,
    #[serde(default)]
    green: Option<String>,
    #[serde(default)]
    blue: Option<String>,
    #[serde(default)]
    amber: Option<String>,
    #[serde(default)]
    accent: Option<String>,
    #[serde(default)]
    teal: Option<String>,
    #[serde(default)]
    red: Option<String>,
    #[serde(default)]
    warn: Option<String>,
    #[serde(default)]
    focus: Option<String>,
    #[serde(default)]
    selection: Option<String>,
}

/// A colour from the file, or the base's own when the field is absent or does
/// not parse.
fn pick(value: &Option<String>, fallback: u32) -> u32 {
    value
        .as_deref()
        .and_then(gui_theme::parse_accent)
        .unwrap_or(fallback)
}

impl PaletteFile {
    /// Paint this file over `base`. An accent that does not read on the
    /// background is lifted the same way a wheel pick is, and the ring and the
    /// selection wash are re-derived with it so they cannot disagree.
    fn over(&self, base: Palette) -> Palette {
        let palette = Palette {
            bg: pick(&self.bg, base.bg),
            panel: pick(&self.panel, base.panel),
            panel_hi: pick(&self.panel_hi, base.panel_hi),
            track: pick(&self.track, base.track),
            text: pick(&self.text, base.text),
            text_strong: pick(&self.text_strong, base.text_strong),
            text_normal: pick(&self.text_normal, base.text_normal),
            text_muted: pick(&self.text_muted, base.text_muted),
            text_title: pick(&self.text_title, base.text_title),
            text_faint: pick(&self.text_faint, base.text_faint),
            text_hint: pick(&self.text_hint, base.text_hint),
            green: pick(&self.green, base.green),
            blue: pick(&self.blue, base.blue),
            amber: pick(&self.amber, base.amber),
            accent: pick(&self.accent, base.accent),
            teal: pick(&self.teal, base.teal),
            red: pick(&self.red, base.red),
            warn: pick(&self.warn, base.warn),
            focus: pick(&self.focus, base.focus),
            selection: pick(&self.selection, base.selection),
        };
        let accent = gui_theme::legible_accent(palette.accent, palette);
        if accent == palette.accent {
            // The file's own ring and wash stand: they were drawn together.
            palette
        } else {
            gui_theme::with_accent(palette, accent)
        }
    }
}

/// Parse one palette file. `None` means the file is not a palette at all (no
/// colours, or a TOML error); the caller logs and moves on.
fn parse(text: &str, base: Palette) -> Option<(Option<String>, Palette)> {
    let file: PaletteFile = toml::from_str(text).ok()?;
    let palette = file.over(base);
    // A file that changes nothing is not a palette: it is an empty file, and
    // offering it would be a card that repaints nothing.
    if palette == base {
        return None;
    }
    Some((file.name, palette))
}

/// Every `*.toml` in `folder` painted over `ORGN`. Split out so a test can
/// point it at a temp dir without touching the real profile.
pub(crate) fn read_into(folder: &Path, into: &mut BTreeMap<String, CatalogEntry>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("gui themes: {} unreadable: {e}", path.display());
                continue;
            }
        };
        match parse(&text, ORGN) {
            Some((name, palette)) => {
                into.insert(
                    stem.to_string(),
                    CatalogEntry {
                        slug: stem.to_string(),
                        label: name.unwrap_or_else(|| stem.to_string()),
                        palette,
                    },
                );
            }
            None => tracing::warn!("gui themes: {} is not a palette; skipped", path.display()),
        }
    }
}

/// Where user palettes live for the active profile.
pub fn dir() -> PathBuf {
    crate::config::profile::resolve_profile_home().join(DIR_NAME)
}

/// Build the whole catalog: built-ins, then the user's folder on top.
fn load() -> BTreeMap<String, CatalogEntry> {
    let mut all: BTreeMap<String, CatalogEntry> = BTreeMap::new();
    for (slug, text) in BUILTIN {
        match parse(text, ORGN) {
            Some((name, palette)) => {
                all.insert(
                    (*slug).to_string(),
                    CatalogEntry {
                        slug: (*slug).to_string(),
                        label: name.unwrap_or_else(|| (*slug).to_string()),
                        palette,
                    },
                );
            }
            None => tracing::warn!("gui themes: built-in '{slug}' does not parse; skipped"),
        }
    }
    read_into(&dir(), &mut all);
    all
}

/// The catalog, held for the frame loop: `lookup` runs on every frame while the
/// modal is open and must not hit the disk.
fn shared() -> &'static RwLock<BTreeMap<String, CatalogEntry>> {
    static MAP: OnceLock<RwLock<BTreeMap<String, CatalogEntry>>> = OnceLock::new();
    MAP.get_or_init(|| RwLock::new(load()))
}

/// The whole catalog, sorted by label so the picker has one stable order.
pub fn catalog() -> Vec<CatalogEntry> {
    let mut rows: Vec<CatalogEntry> = match shared().read() {
        Ok(map) => map.values().cloned().collect(),
        // A poisoned lock must not blank the window: rebuild from the embedded
        // catalog, which cannot fail.
        Err(_) => {
            let mut fresh = BTreeMap::new();
            read_into(&dir(), &mut fresh);
            fresh.into_values().collect()
        }
    };
    rows.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    rows
}

/// The palette `[gui] palette` names, if the catalog still has it.
pub fn lookup(slug: &str) -> Option<Palette> {
    shared()
        .read()
        .ok()
        .and_then(|map| map.get(slug).map(|entry| entry.palette))
}

/// Drop the cached catalog, so the next frame sees a file added or edited.
pub fn reload() {
    if let Ok(mut map) = shared().write() {
        *map = load();
    }
}
