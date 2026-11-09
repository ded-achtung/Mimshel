//! Session keyboard settings, read once before creating the shared seat keymap.
use std::process::{Command, Stdio};

#[derive(Default)]
pub(crate) struct Settings {
    pub(crate) layout: String,
    variant: String,
    options: String,
}
fn output(program: &str, args: &[&str]) -> Option<String> {
    let result = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
}
fn xfconf(property: &str) -> Option<String> {
    output("xfconf-query", &["-c", "keyboard-layout", "-p", property])
}
impl Settings {
    pub(crate) fn read() -> Self {
        if xfconf("/Default/XkbDisable").as_deref() != Some("true")
            && let Some(layout) = xfconf("/Default/XkbLayout").filter(|s| !s.is_empty())
        {
            return Self {
                layout,
                variant: xfconf("/Default/XkbVariant").unwrap_or_default(),
                options: [
                    xfconf("/Default/XkbOptions/Group"),
                    xfconf("/Default/XkbOptions/Compose"),
                ]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(","),
            };
        }
        if let Some(query) = output("localectl", &["status", "--no-pager"]) {
            let settings = Self::parse(&query, "X11 Layout", "X11 Variant", "X11 Options");
            if !settings.layout.is_empty() {
                return settings;
            }
        }
        if let Some(query) = output("setxkbmap", &["-query"]) {
            let settings = Self::parse(&query, "layout", "variant", "options");
            if !settings.layout.is_empty() {
                return settings;
            }
        }
        Self {
            layout: "us".into(),
            ..Self::default()
        }
    }
    fn parse(query: &str, layout: &str, variant: &str, options: &str) -> Self {
        let field = |name| {
            query
                .lines()
                .filter_map(|s| s.split_once(':'))
                .find(|(key, _)| key.trim() == name)
                .map(|(_, value)| value.trim().to_owned())
                .unwrap_or_default()
        };
        Self {
            layout: field(layout),
            variant: field(variant),
            options: field(options),
        }
    }
    pub(crate) fn config(&self) -> smithay::input::keyboard::XkbConfig<'_> {
        smithay::input::keyboard::XkbConfig {
            layout: &self.layout,
            variant: &self.variant,
            options: Some(self.options.clone()),
            ..Default::default()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_queries_keep_variants_and_switch_options() {
        let settings = Settings::parse(
            "layout: us,ru\nvariant: ,phonetic\noptions: grp:alt_shift_toggle,compose:ralt\n",
            "layout",
            "variant",
            "options",
        );
        assert_eq!(settings.layout, "us,ru");
        assert_eq!(settings.variant, ",phonetic");
        assert_eq!(settings.options, "grp:alt_shift_toggle,compose:ralt");
        let settings = Settings::parse(
            " X11 Layout: us,ru\n X11 Options: grp:alt_shift_toggle\n",
            "X11 Layout",
            "X11 Variant",
            "X11 Options",
        );
        assert_eq!(settings.layout, "us,ru");
        assert_eq!(settings.options, "grp:alt_shift_toggle");
    }
}
