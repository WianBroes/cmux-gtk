use gtk4::gdk::{Key, ModifierType};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level config loaded from ~/.config/cmux/config.toml.
/// Missing sections retain their built-in defaults.
#[derive(serde::Deserialize, Default, Debug)]
pub struct Config {
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

/// Per-action shortcut overrides. Each value is a GTK accelerator string (e.g. "<Ctrl>n").
/// None means "use default".
#[derive(serde::Deserialize, Default, Debug)]
pub struct ShortcutConfig {
    pub new_workspace: Option<String>,
    pub close_workspace: Option<String>,
    pub next_workspace: Option<String>,
    pub prev_workspace: Option<String>,
    pub move_workspace_up: Option<String>,
    pub move_workspace_down: Option<String>,
    pub toggle_workspace_group: Option<String>,
    pub rename_workspace: Option<String>,
    pub toggle_sidebar: Option<String>,
    pub toggle_right_sidebar: Option<String>,
    pub focus_right_sidebar: Option<String>,
    pub split_right: Option<String>,
    pub split_down: Option<String>,
    pub close_pane: Option<String>,
    pub new_ssh_workspace: Option<String>,
    pub focus_left: Option<String>,
    pub focus_right: Option<String>,
    pub focus_up: Option<String>,
    pub focus_down: Option<String>,
    pub focus_back: Option<String>,
    pub focus_forward: Option<String>,
    pub workspace_1: Option<String>,
    pub workspace_2: Option<String>,
    pub workspace_3: Option<String>,
    pub workspace_4: Option<String>,
    pub workspace_5: Option<String>,
    pub workspace_6: Option<String>,
    pub workspace_7: Option<String>,
    pub workspace_8: Option<String>,
    pub workspace_9: Option<String>,
    pub browser_open: Option<String>,
    pub browser_close: Option<String>,
}

/// UI configuration section -- [ui] in config.toml (D-16).
#[derive(serde::Deserialize, Default, Debug)]
pub struct UiConfig {
    #[serde(default)]
    pub header_bar: HeaderBarConfig,
}

/// Header bar configuration -- [ui.header_bar] in config.toml (D-16).
/// Requires app restart to take effect.
#[derive(serde::Deserialize, Debug)]
pub struct HeaderBarConfig {
    /// "none" hides the header; "gtk" and other values use the standard GTK header.
    #[serde(default = "default_header_style")]
    pub style: String,
}

/// Supply the visible GTK header when the setting is omitted.
fn default_header_style() -> String {
    "gtk".to_string()
}

impl Default for HeaderBarConfig {
    /// Preserve a visible header for configurations without UI settings.
    fn default() -> Self {
        Self {
            style: default_header_style(),
        }
    }
}

/// All bindable shortcut actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    NewWorkspace,
    CloseWorkspace,
    NextWorkspace,
    PrevWorkspace,
    MoveWorkspaceUp,
    MoveWorkspaceDown,
    ToggleWorkspaceGroup,
    RenameWorkspace,
    ToggleSidebar,
    ToggleRightSidebar,
    FocusRightSidebar,
    SplitRight,
    SplitDown,
    ClosePane,
    NewSshWorkspace,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    FocusBack,
    FocusForward,
    Workspace1,
    Workspace2,
    Workspace3,
    Workspace4,
    Workspace5,
    Workspace6,
    Workspace7,
    Workspace8,
    Workspace9,
    BrowserOpen,
    BrowserClose,
}

thread_local! {
    /// The live shortcut map shared with the key handler; `reload_shortcuts` swaps its contents.
    static LIVE_MAP: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<ShortcutMap>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Publish the shortcut map the key handler reads, so a reload can replace it in place.
pub fn set_live_map(map: std::rc::Rc<std::cell::RefCell<ShortcutMap>>) {
    LIVE_MAP.with(|live| *live.borrow_mut() = Some(map));
}

/// Re-read `config.toml` + `cmux.json`, rebuild the shortcut map and re-register the menu
/// accelerators. Returns false when the UI has not published a map yet. GTK main thread only.
/// Header style and other startup-only settings still need a restart.
pub fn reload_shortcuts() -> bool {
    let Some(live) = LIVE_MAP.with(|live| live.borrow().clone()) else {
        return false;
    };
    let fresh = ShortcutMap::from_config(&load_config().shortcuts);
    use gtk4::prelude::Cast;
    if let Some(app) = gtk4::gio::Application::default().and_then(|a| a.downcast::<gtk4::Application>().ok()) {
        crate::menus::register_accels(&app, &fresh);
    }
    *live.borrow_mut() = fresh;
    true
}

/// The action bound to a key combination in the live map, if any (for conflict checks).
pub fn live_lookup(mods: ModifierType, key: Key) -> Option<ShortcutAction> {
    let live = LIVE_MAP.with(|live| live.borrow().clone())?;
    let action = live.borrow().lookup(mods, key);
    action
}

/// GTK spelling of the key the live map gives an action; `None` when unbound.
pub fn live_accelerator(action: ShortcutAction) -> Option<String> {
    let live = LIVE_MAP.with(|live| live.borrow().clone())?;
    let accelerator = live.borrow().accelerator_for(action);
    accelerator
}

/// Shortcuts editable from the settings page: `cmux.json` action id, label, section, action.
/// This is the table behind [`shortcut_slot`]; other actions are still set in `config.toml`.
pub const EDITABLE_SHORTCUTS: &[(&str, &str, &str, ShortcutAction)] = &[
    ("newTab", "New workspace", "Workspaces", ShortcutAction::NewWorkspace),
    ("closeWorkspace", "Close workspace", "Workspaces", ShortcutAction::CloseWorkspace),
    ("nextSidebarTab", "Next workspace", "Workspaces", ShortcutAction::NextWorkspace),
    ("prevSidebarTab", "Previous workspace", "Workspaces", ShortcutAction::PrevWorkspace),
    ("moveWorkspaceUp", "Move workspace up", "Workspaces", ShortcutAction::MoveWorkspaceUp),
    ("moveWorkspaceDown", "Move workspace down", "Workspaces", ShortcutAction::MoveWorkspaceDown),
    (
        "toggleFocusedWorkspaceGroupCollapsed",
        "Collapse / expand workspace group",
        "Workspaces",
        ShortcutAction::ToggleWorkspaceGroup,
    ),
    ("renameWorkspace", "Rename workspace", "Workspaces", ShortcutAction::RenameWorkspace),
    ("splitRight", "Split right", "Panes", ShortcutAction::SplitRight),
    ("splitDown", "Split down", "Panes", ShortcutAction::SplitDown),
    ("focusLeft", "Focus pane left", "Panes", ShortcutAction::FocusLeft),
    ("focusRight", "Focus pane right", "Panes", ShortcutAction::FocusRight),
    ("focusUp", "Focus pane up", "Panes", ShortcutAction::FocusUp),
    ("focusDown", "Focus pane down", "Panes", ShortcutAction::FocusDown),
    ("focusHistoryBack", "Focus history back", "Panes", ShortcutAction::FocusBack),
    ("focusHistoryForward", "Focus history forward", "Panes", ShortcutAction::FocusForward),
    ("toggleSidebar", "Toggle sidebar", "Sidebars", ShortcutAction::ToggleSidebar),
    ("focusRightSidebar", "Focus file explorer", "Sidebars", ShortcutAction::FocusRightSidebar),
    ("openBrowser", "Open browser", "Browser", ShortcutAction::BrowserOpen),
];

/// Combinations wired directly to menu actions, not editable, that a new shortcut must not take.
pub const FIXED_SHORTCUTS: &[(&str, &str)] = &[
    ("<Ctrl>t", "New terminal tab"),
    ("<Ctrl><Shift>l", "New browser tab"),
    ("<Ctrl><Shift>c", "Copy"),
    ("<Ctrl><Shift>v", "Paste"),
    ("<Ctrl>f", "Find"),
    ("<Ctrl>comma", "Preferences"),
    ("<Ctrl><Shift>i", "Notifications"),
    ("<Ctrl>q", "Quit"),
];

/// Spell a captured key as a `cmux.json` shortcut ("ctrl+shift+pageup"); the exact inverse of
/// [`to_gtk_accelerator`] for the keys the settings page accepts.
pub fn accelerator_to_text(mods: ModifierType, key: Key) -> String {
    let mut parts: Vec<String> = Vec::new();
    if mods.contains(ModifierType::CONTROL_MASK) {
        parts.push("ctrl".into());
    }
    if mods.contains(ModifierType::SHIFT_MASK) {
        parts.push("shift".into());
    }
    if mods.contains(ModifierType::ALT_MASK) {
        parts.push("alt".into());
    }
    let name = key.name().map(|n| n.to_string()).unwrap_or_default();
    parts.push(match name.as_str() {
        "bracketleft" => "[".into(),
        "bracketright" => "]".into(),
        "Page_Up" => "pageup".into(),
        "Page_Down" => "pagedown".into(),
        "Return" | "KP_Enter" => "enter".into(),
        other => other.to_ascii_lowercase(),
    });
    parts.join("+")
}

/// HashMap-based shortcut lookup table built from config + defaults.
pub struct ShortcutMap {
    map: HashMap<(ModifierType, Key), ShortcutAction>,
}

/// Known shortcut action names for unknown-key detection.
const KNOWN_SHORTCUTS: &[&str] = &[
    "new_workspace",
    "close_workspace",
    "next_workspace",
    "prev_workspace",
    "move_workspace_up",
    "move_workspace_down",
    "toggle_workspace_group",
    "rename_workspace",
    "toggle_sidebar",
    "toggle_right_sidebar",
    "focus_right_sidebar",
    "split_right",
    "split_down",
    "close_pane",
    "new_ssh_workspace",
    "focus_left",
    "focus_right",
    "focus_up",
    "focus_down",
    "focus_back",
    "focus_forward",
    "workspace_1",
    "workspace_2",
    "workspace_3",
    "workspace_4",
    "workspace_5",
    "workspace_6",
    "workspace_7",
    "workspace_8",
    "workspace_9",
    "browser_open",
    "browser_close",
];

/// Modifier mask for lookup: ignore Caps Lock, Num Lock, etc.
const MOD_MASK: ModifierType = ModifierType::from_bits_truncate(
    ModifierType::CONTROL_MASK.bits()
        | ModifierType::SHIFT_MASK.bits()
        | ModifierType::ALT_MASK.bits(),
);

/// Returns the config file path.
/// Respects $XDG_CONFIG_HOME/cmux/config.toml; falls back to ~/.config/cmux/config.toml (CFG-04).
pub fn config_path() -> PathBuf {
    cmux_platform::paths::config_dir().join("config.toml")
}

/// Load config from disk. Always returns a usable Config (D-10).
/// Missing file is silent; read/parse errors warn to stderr and fall back to defaults.
pub fn load_config() -> Config {
    let path = config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("cmux: config read error at {}: {e}", path.display());
            }
            let mut config = Config::default();
            overlay_cmux_json(&mut config);
            return config;
        }
    };

    warn_unknown_shortcuts(&content);

    let mut config = match toml::from_str::<Config>(&content) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("cmux: config parse error at {}: {e}", path.display());
            Config::default()
        }
    };
    overlay_cmux_json(&mut config);
    config
}

/// Apply the global `cmux.json` on top of `config.toml`: a key set in both takes the JSON value.
/// An unreadable or invalid file is reported on stderr and ignored; the TOML values stay.
fn overlay_cmux_json(config: &mut Config) {
    let Some(path) = crate::settings_json::global_path() else {
        return;
    };
    match crate::settings_json::read(&path) {
        Ok(value) => apply_cmux_json(config, &value),
        Err(e) => eprintln!("cmux: cmux.json read error: {e}"),
    }
}

/// Map the `cmux.json` keys the Linux app honors onto `Config`.
///
/// Only `shortcuts.bindings.<actionId>` with a single shortcut string is mapped (table in
/// [`shortcut_slot`]); chords and ids without a Linux action are ignored; `null`/`""`/`none`/… unbind.
/// Other sections stay CLI-only (`cmux config get|set`) until the app has a setting for them.
fn apply_cmux_json(config: &mut Config, value: &serde_json::Value) {
    let Some(bindings) = value
        .pointer("/shortcuts/bindings")
        .and_then(|v| v.as_object())
    else {
        return;
    };
    for (id, binding) in bindings {
        let Some(slot) = shortcut_slot(&mut config.shortcuts, id) else {
            continue;
        };
        // Unbinding spellings from the schema: null, "", none, clear, unbound, disabled. An
        // empty accelerator in `ShortcutConfig` means "no key for this action".
        if binding.is_null()
            || binding.as_str().is_some_and(|s| {
                matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "" | "none" | "clear" | "unbound" | "disabled"
                )
            })
        {
            *slot = Some(String::new());
            continue;
        }
        let Some(text) = binding.as_str() else {
            continue;
        };
        *slot = Some(to_gtk_accelerator(text));
    }
}

/// The `ShortcutConfig` field driven by a `cmux.json` action id, if the Linux app has one.
fn shortcut_slot<'a>(cfg: &'a mut ShortcutConfig, id: &str) -> Option<&'a mut Option<String>> {
    Some(match id {
        "newTab" => &mut cfg.new_workspace,
        "closeWorkspace" => &mut cfg.close_workspace,
        "nextSidebarTab" => &mut cfg.next_workspace,
        "prevSidebarTab" => &mut cfg.prev_workspace,
        "moveWorkspaceUp" => &mut cfg.move_workspace_up,
        "moveWorkspaceDown" => &mut cfg.move_workspace_down,
        "toggleFocusedWorkspaceGroupCollapsed" => &mut cfg.toggle_workspace_group,
        "renameWorkspace" => &mut cfg.rename_workspace,
        "toggleSidebar" => &mut cfg.toggle_sidebar,
        "focusRightSidebar" => &mut cfg.focus_right_sidebar,
        "splitRight" => &mut cfg.split_right,
        "splitDown" => &mut cfg.split_down,
        "focusLeft" => &mut cfg.focus_left,
        "focusRight" => &mut cfg.focus_right,
        "focusUp" => &mut cfg.focus_up,
        "focusDown" => &mut cfg.focus_down,
        "focusHistoryBack" => &mut cfg.focus_back,
        "focusHistoryForward" => &mut cfg.focus_forward,
        "openBrowser" => &mut cfg.browser_open,
        _ => return None,
    })
}

/// Turn a `cmux.json` shortcut ("cmd+shift+b") into a GTK accelerator ("<Ctrl><Shift>b").
/// `cmd`/`ctrl` both mean Ctrl (Linux has no Command key); an invalid result is caught later by
/// `ShortcutMap::from_config`, which warns and keeps the default.
fn to_gtk_accelerator(text: &str) -> String {
    let mut out = String::new();
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = parts.pop().unwrap_or_default();
    for m in parts {
        out.push_str(match m.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "ctrl" | "control" => "<Ctrl>",
            "shift" => "<Shift>",
            "opt" | "option" | "alt" => "<Alt>",
            _ => "<Unknown>",
        });
    }
    out.push_str(match key.to_ascii_lowercase().as_str() {
        "[" => "bracketleft",
        "]" => "bracketright",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "pageup" => "Page_Up",
        "pagedown" => "Page_Down",
        "tab" => "Tab",
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "space" => "space",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        "delete" => "Delete",
        "backspace" => "BackSpace",
        lower => {
            // Function keys are spelled F1..F35 by GDK, which is case sensitive.
            let function = lower
                .strip_prefix('f')
                .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            return match function {
                Some(n) => format!("{out}F{n}"),
                None => out + lower,
            };
        }
    });
    out
}

/// Warn about unknown keys in the [shortcuts] table (D-03).
fn warn_unknown_shortcuts(content: &str) {
    let table: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return, // Parse errors are reported by load_config
    };
    if let Some(shortcuts) = table.get("shortcuts").and_then(|v| v.as_table()) {
        for key in shortcuts.keys() {
            if !KNOWN_SHORTCUTS.contains(&key.as_str()) {
                eprintln!(
                    "cmux: unknown shortcut action '{}' in config, ignoring",
                    key
                );
            }
        }
    }
}

impl ShortcutMap {
    /// Build lookup table from config, falling back to defaults for unset/invalid entries.
    pub fn from_config(config: &ShortcutConfig) -> Self {
        let entries: &[(ShortcutAction, &Option<String>, &str)] = &[
            (
                ShortcutAction::NewWorkspace,
                &config.new_workspace,
                "<Ctrl>n",
            ),
            (
                ShortcutAction::CloseWorkspace,
                &config.close_workspace,
                "<Ctrl><Shift>w",
            ),
            (
                ShortcutAction::NextWorkspace,
                &config.next_workspace,
                "<Ctrl>bracketright",
            ),
            (
                ShortcutAction::PrevWorkspace,
                &config.prev_workspace,
                "<Ctrl>bracketleft",
            ),
            (
                ShortcutAction::RenameWorkspace,
                &config.rename_workspace,
                "<Ctrl><Shift>r",
            ),
            (
                ShortcutAction::MoveWorkspaceUp,
                &config.move_workspace_up,
                "<Ctrl><Shift>Page_Up",
            ),
            (
                ShortcutAction::MoveWorkspaceDown,
                &config.move_workspace_down,
                "<Ctrl><Shift>Page_Down",
            ),
            (
                ShortcutAction::ToggleWorkspaceGroup,
                &config.toggle_workspace_group,
                "<Ctrl><Alt>g",
            ),
            (
                ShortcutAction::ToggleSidebar,
                &config.toggle_sidebar,
                "<Ctrl>b",
            ),
            // Upstream ⌘⌥B: toggle the right sidebar (the Files panel).
            (
                ShortcutAction::ToggleRightSidebar,
                &config.toggle_right_sidebar,
                "<Ctrl><Alt>b",
            ),
            // Upstream ⌘⇧E: toggle focus between the panel and the terminal.
            (
                ShortcutAction::FocusRightSidebar,
                &config.focus_right_sidebar,
                "<Ctrl><Shift>e",
            ),
            (ShortcutAction::SplitRight, &config.split_right, "<Ctrl>d"),
            (
                ShortcutAction::SplitDown,
                &config.split_down,
                "<Ctrl><Shift>d",
            ),
            (
                ShortcutAction::ClosePane,
                &config.close_pane,
                "<Ctrl><Shift>x",
            ),
            (
                ShortcutAction::NewSshWorkspace,
                &config.new_ssh_workspace,
                "<Ctrl><Shift>s",
            ),
            (
                ShortcutAction::FocusLeft,
                &config.focus_left,
                "<Ctrl><Shift>Left",
            ),
            (
                ShortcutAction::FocusRight,
                &config.focus_right,
                "<Ctrl><Shift>Right",
            ),
            (ShortcutAction::FocusUp, &config.focus_up, "<Ctrl><Shift>Up"),
            (
                ShortcutAction::FocusDown,
                &config.focus_down,
                "<Ctrl><Shift>Down",
            ),
            // Upstream's Focus Back / Forward are Cmd+[ / Cmd+]; Ctrl+[ / ] already switch
            // workspaces here (and Ctrl+[ is Escape in a terminal).
            (
                ShortcutAction::FocusBack,
                &config.focus_back,
                "<Ctrl><Alt>Left",
            ),
            (
                ShortcutAction::FocusForward,
                &config.focus_forward,
                "<Ctrl><Alt>Right",
            ),
            (ShortcutAction::Workspace1, &config.workspace_1, "<Ctrl>1"),
            (ShortcutAction::Workspace2, &config.workspace_2, "<Ctrl>2"),
            (ShortcutAction::Workspace3, &config.workspace_3, "<Ctrl>3"),
            (ShortcutAction::Workspace4, &config.workspace_4, "<Ctrl>4"),
            (ShortcutAction::Workspace5, &config.workspace_5, "<Ctrl>5"),
            (ShortcutAction::Workspace6, &config.workspace_6, "<Ctrl>6"),
            (ShortcutAction::Workspace7, &config.workspace_7, "<Ctrl>7"),
            (ShortcutAction::Workspace8, &config.workspace_8, "<Ctrl>8"),
            (ShortcutAction::Workspace9, &config.workspace_9, "<Ctrl>9"),
            (
                ShortcutAction::BrowserOpen,
                &config.browser_open,
                "<Ctrl><Shift>b",
            ),
            (
                ShortcutAction::BrowserClose,
                &config.browser_close,
                "<Ctrl><Shift>q",
            ),
        ];

        let mut map = HashMap::new();

        for (action, config_val, default_accel) in entries {
            let accel_str = config_val.as_deref().unwrap_or(*default_accel);
            if accel_str.is_empty() {
                continue; // explicitly unbound
            }
            let action_name = format!("{:?}", action);

            if let Some((key, mods)) = gtk4::accelerator_parse(accel_str) {
                map.insert((mods & MOD_MASK, key), *action);
            } else {
                // D-11: invalid accelerator — warn and use default
                eprintln!(
                    "cmux: invalid shortcut '{}' for {}, using default '{}'",
                    accel_str, action_name, default_accel
                );
                if let Some((key, mods)) = gtk4::accelerator_parse(*default_accel) {
                    map.insert((mods & MOD_MASK, key), *action);
                }
            }
        }

        ShortcutMap { map }
    }

    /// Return GTK's accelerator spelling for an action surviving config and collision resolution.
    /// Menus use this same map as capture-phase dispatch, avoiding stale default accelerators.
    pub fn accelerator_for(&self, action: ShortcutAction) -> Option<String> {
        self.map.iter().find_map(|((mods, key), mapped)| {
            (*mapped == action).then(|| gtk4::accelerator_name(*key, *mods).to_string())
        })
    }

    /// Look up a shortcut action for the given modifier+key combination.
    /// Masks modifiers to ignore Caps Lock, Num Lock, etc.
    /// Normalizes keyval to lowercase because GTK4 key events give uppercase
    /// when Shift is held (e.g. Key::R), but accelerator_parse stores lowercase
    /// with the Shift modifier flag (e.g. Key::r + SHIFT_MASK).
    pub fn lookup(&self, mods: ModifierType, key: Key) -> Option<ShortcutAction> {
        let masked = mods & MOD_MASK;
        let lower_key = key.to_lower();
        self.map.get(&(masked, lower_key)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve configuration under an explicitly set XDG directory.
    #[test]
    fn test_config_path_xdg() {
        // Temporarily set XDG_CONFIG_HOME and verify config_path() uses it.
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/test-xdg-config");
        let path = config_path();
        assert_eq!(path, PathBuf::from("/tmp/test-xdg-config/cmux/config.toml"));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    /// Use built-in defaults when no configuration file exists.
    #[test]
    fn test_load_config_missing_file() {
        // Point to a nonexistent dir so load_config returns defaults silently.
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/cmux-test-nonexistent-dir-xyz");
        let config = load_config();
        assert!(config.shortcuts.new_workspace.is_none());
        assert!(config.shortcuts.close_workspace.is_none());
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    /// Accept an empty configuration without losing shortcut defaults.
    #[test]
    fn test_load_config_empty_file() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-empty-{}", std::process::id()));
        let cfg_dir = dir.join("cmux");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("config.toml");
        std::fs::write(&cfg_file, "").unwrap();

        std::env::set_var("XDG_CONFIG_HOME", dir.to_str().unwrap());
        let config = load_config();
        assert!(config.shortcuts.new_workspace.is_none());
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Read a configured accelerator from the real TOML file path.
    #[test]
    fn test_load_config_valid_shortcuts() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-valid-{}", std::process::id()));
        let cfg_dir = dir.join("cmux");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("config.toml");
        std::fs::write(&cfg_file, "[shortcuts]\nnew_workspace = \"<Ctrl>t\"\n").unwrap();

        std::env::set_var("XDG_CONFIG_HOME", dir.to_str().unwrap());
        let config = load_config();
        assert_eq!(config.shortcuts.new_workspace, Some("<Ctrl>t".to_string()));
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fall back to defaults when configuration syntax is invalid.
    #[test]
    fn test_load_config_invalid_toml() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-invalid-{}", std::process::id()));
        let cfg_dir = dir.join("cmux");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("config.toml");
        std::fs::write(&cfg_file, "[shortcuts\n").unwrap();

        std::env::set_var("XDG_CONFIG_HOME", dir.to_str().unwrap());
        let config = load_config();
        // Falls back to defaults on parse error
        assert!(config.shortcuts.new_workspace.is_none());
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cmux.json` bindings override the TOML value; chords and unknown ids are ignored; null unbinds.
    #[test]
    fn test_apply_cmux_json_bindings() {
        let mut config: Config =
            toml::from_str("[shortcuts]\nnew_workspace = \"<Ctrl>t\"\nsplit_down = \"<Ctrl>j\"\n")
                .unwrap();
        let json: serde_json::Value = serde_json::from_str(
            r#"{"shortcuts":{"bindings":{"newTab":"cmd+shift+n","splitRight":["ctrl+b","c"],
            "splitDown":null,"focusHistoryBack":"ctrl+alt+[","noSuchAction":"ctrl+q"}}}"#,
        )
        .unwrap();
        apply_cmux_json(&mut config, &json);
        assert_eq!(config.shortcuts.new_workspace.as_deref(), Some("<Ctrl><Shift>n"));
        assert_eq!(config.shortcuts.split_right, None);
        assert_eq!(config.shortcuts.split_down.as_deref(), Some(""));
        assert_eq!(
            config.shortcuts.focus_back.as_deref(),
            Some("<Ctrl><Alt>bracketleft")
        );
    }

    /// Named keys map to GTK keysym names.
    #[test]
    fn test_to_gtk_accelerator() {
        assert_eq!(to_gtk_accelerator("ctrl+shift+PageUp"), "<Ctrl><Shift>Page_Up");
        assert_eq!(to_gtk_accelerator("alt+Left"), "<Alt>Left");
        assert_eq!(to_gtk_accelerator("b"), "b");
    }

    /// Keep the GTK header visible when UI settings are omitted.
    #[test]
    fn test_ui_config_default() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.ui.header_bar.style, "gtk");
    }

    /// Accept the hidden-header setting alongside ignored legacy button keys.
    #[test]
    fn test_ui_config_hidden_header_accepts_legacy_keys() {
        let config: Config = toml::from_str(
            r#"
[ui.header_bar]
style = "none"
buttons_left = ["new_workspace"]
buttons_right = ["split_right", "toggle_sidebar"]
"#,
        )
        .unwrap();
        assert_eq!(config.ui.header_bar.style, "none");
    }

    // Tests that require GTK4 initialization (accelerator_parse).
    // These will only work in environments with a display (or virtual display).

    /// Resolve the built-in new-workspace accelerator through GTK parsing.
    #[test]
    fn test_shortcut_map_defaults() {
        if gtk4::init().is_err() {
            eprintln!("Skipping test_shortcut_map_defaults: GTK4 init failed (headless)");
            return;
        }
        cmux_json_unbind_spellings_leave_no_key_for_the_action();
        captured_shortcuts_round_trip_through_cmux_json_text();
        let smap = ShortcutMap::from_config(&ShortcutConfig::default());
        // Ctrl+N should map to NewWorkspace
        let result = smap.lookup(ModifierType::CONTROL_MASK, Key::n);
        assert_eq!(result, Some(ShortcutAction::NewWorkspace));
        // Upstream ⌘⌥B becomes Ctrl+Alt+B and must collide with no other default.
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, Key::b),
            Some(ShortcutAction::ToggleRightSidebar)
        );
    }

    /// Replace the default accelerator when a configured shortcut overrides it.
    #[test]
    fn test_shortcut_map_custom() {
        if gtk4::init().is_err() {
            eprintln!("Skipping test_shortcut_map_custom: GTK4 init failed (headless)");
            return;
        }
        let config = ShortcutConfig {
            new_workspace: Some("<Ctrl>t".to_string()),
            ..Default::default()
        };
        let smap = ShortcutMap::from_config(&config);
        // Ctrl+T should now map to NewWorkspace
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK, Key::t),
            Some(ShortcutAction::NewWorkspace)
        );
        // Ctrl+N should no longer map to NewWorkspace
        assert_eq!(smap.lookup(ModifierType::CONTROL_MASK, Key::n), None);
    }

    /// Needs GTK: called from `test_shortcut_map_defaults`, the one test thread that owns GTK.
    fn cmux_json_unbind_spellings_leave_no_key_for_the_action() {
        for spelling in [
            serde_json::json!(null),
            serde_json::json!(""),
            serde_json::json!("none"),
            serde_json::json!("Unbound"),
            serde_json::json!("disabled"),
        ] {
            let mut config = Config::default();
            let doc = serde_json::json!({"shortcuts": {"bindings": {"splitRight": spelling}}});
            apply_cmux_json(&mut config, &doc);
            assert_eq!(config.shortcuts.split_right.as_deref(), Some(""));
            let map = ShortcutMap::from_config(&config.shortcuts);
            assert_eq!(map.accelerator_for(ShortcutAction::SplitRight), None);
            // Other actions keep their defaults.
            assert!(map.accelerator_for(ShortcutAction::SplitDown).is_some());
        }
    }

    /// Needs GTK: called from `test_shortcut_map_defaults`, the one test thread that owns GTK.
    fn captured_shortcuts_round_trip_through_cmux_json_text() {
        let cases = [
            (ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK, Key::k),
            (ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, Key::Left),
            (ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK, Key::Page_Up),
            (ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, Key::bracketleft),
            (ModifierType::ALT_MASK, Key::F5),
            (ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, Key::comma),
            (ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, Key::Return),
            (ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, Key::Home),
        ];
        for (mods, key) in cases {
            let text = accelerator_to_text(mods, key);
            let accel = to_gtk_accelerator(&text);
            let (parsed_key, parsed_mods) = gtk4::accelerator_parse(&accel)
                .unwrap_or_else(|| panic!("'{text}' -> '{accel}' does not parse"));
            assert_eq!((parsed_key, parsed_mods & MOD_MASK), (key.to_lower(), mods), "{text}");
        }
    }

}
