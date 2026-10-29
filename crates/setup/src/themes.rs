//! Installed themes available to the settings window.

use std::path::{Path, PathBuf};

/// Installed GTK theme folder names.
pub fn gtk_themes() -> Vec<String> {
    gtk_themes_in(&bases("themes", ".themes"))
}

/// Installed, visible icon theme folder names.
pub fn icon_themes() -> Vec<String> {
    icon_themes_in(&bases("icons", ".icons"))
}

/// Installed cursor theme folder names.
pub fn cursor_themes() -> Vec<String> {
    cursor_themes_in(&bases("icons", ".icons"))
}

/// Where the GTK theme `name` is installed: the first place that has it,
/// in the order GTK looks (`~/.themes` first).
pub fn gtk_theme_dir(name: &str) -> Option<PathBuf> {
    gtk_theme_dir_in(&bases("themes", ".themes"), name)
}

fn gtk_theme_dir_in(bases: &[PathBuf], name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    bases
        .iter()
        .map(|b| b.join(name))
        .find(|p| is_gtk_theme(p) || p.join("index.theme").is_file())
}

fn is_gtk_theme(p: &Path) -> bool {
    p.join("gtk-3.0/gtk.css").is_file() || p.join("gtk-4.0/gtk.css").is_file()
}

/// Whether an executable program can be found on PATH.
pub fn installed(program: &str) -> bool {
    crate::sys::which(program).is_some()
}

fn bases(kind: &str, legacy: &str) -> Vec<PathBuf> {
    let home = crate::sys::home();
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut bases = vec![home.join(legacy), data.join(kind)];
    bases.extend(
        std::env::split_paths(&dirs)
            .filter(|p| p.is_absolute())
            .map(|p| p.join(kind)),
    );
    bases
}

fn scan(bases: &[PathBuf], matches: impl Fn(&Path) -> bool) -> Vec<String> {
    let mut names: Vec<String> = bases
        .iter()
        .flat_map(|base| std::fs::read_dir(base).into_iter().flatten().flatten())
        .filter(|entry| entry.path().is_dir() && matches(&entry.path()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    // As a person reads a list: «adwaita» next to «Adwaita-dark», not
    // after every name with a capital.
    names.sort_by_key(|n| (n.to_lowercase(), n.clone()));
    names.dedup();
    names
}

fn gtk_themes_in(bases: &[PathBuf]) -> Vec<String> {
    scan(bases, is_gtk_theme)
}

fn icon_themes_in(bases: &[PathBuf]) -> Vec<String> {
    scan(bases, |p| {
        if p.file_name().is_some_and(|n| n == "default") {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(p.join("index.theme")) else {
            return false;
        };
        let mut section = false;
        let mut directories = false;
        let mut hidden = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line == "[Icon Theme]";
            } else if section && let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "Directories" => directories = value.split(',').any(|v| !v.trim().is_empty()),
                    "Hidden" => hidden = value.trim() == "true",
                    _ => {}
                }
            }
        }
        directories && !hidden
    })
}

fn cursor_themes_in(bases: &[PathBuf]) -> Vec<String> {
    scan(bases, |p| p.join("cursors").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_folders_are_filtered_sorted_and_deduplicated() {
        let root = std::env::temp_dir().join(format!("mimshel-themes-{}", std::process::id()));
        let bases = [root.join("a"), root.join("b"), root.join("missing")];
        for (base, name, file, content) in [
            (0, "Zgtk", "gtk-3.0/gtk.css", ""),
            (1, "Zgtk", "gtk-4.0/gtk.css", ""),
            (0, "Agtk", "gtk-4.0/gtk.css", ""),
            (0, "Incomplete", "gtk-3.0/other", ""),
            (
                0,
                "Icons",
                "index.theme",
                "[Icon Theme]\nDirectories=48/apps\n",
            ),
            (
                1,
                "Icons",
                "index.theme",
                "[Icon Theme]\nDirectories=48/apps\n",
            ),
            (
                0,
                "Hidden",
                "index.theme",
                "[Icon Theme]\nDirectories=48/apps\nHidden=true\n",
            ),
            (0, "Cursor", "cursors/left_ptr", ""),
            (0, "Cursor", "index.theme", "[Icon Theme]\nName=Cursor\n"),
            (
                0,
                "default",
                "index.theme",
                "[Icon Theme]\nDirectories=48/apps\n",
            ),
            (
                0,
                "hicolor",
                "index.theme",
                "[Icon Theme]\nDirectories=48/apps\n",
            ),
            (0, "Empty", "index.theme", "[Icon Theme]\nDirectories= , \n"),
            (
                0,
                "WrongSection",
                "index.theme",
                "[Other]\nDirectories=48/apps\n",
            ),
        ] {
            let path = bases[base].join(name).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        assert_eq!(gtk_themes_in(&bases), ["Agtk", "Zgtk"]);
        assert_eq!(icon_themes_in(&bases), ["hicolor", "Icons"]);
        assert_eq!(cursor_themes_in(&bases), ["Cursor"]);
        // The first place wins; a name is never a path.
        assert_eq!(
            gtk_theme_dir_in(&bases, "Zgtk"),
            Some(bases[0].join("Zgtk"))
        );
        assert_eq!(gtk_theme_dir_in(&bases, "Incomplete"), None);
        assert_eq!(gtk_theme_dir_in(&bases, "../a/Zgtk"), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
