//! Provider marks for terminal tabs, as upstream's `TerminalTabAgentIconResolver`: the tab of a
//! terminal running a known agent shows that agent's icon before its title. Icons come from
//! upstream's `Assets.xcassets/AgentIcons` (dark variants), see `resources/agent-icons/`.

use gtk4::{gdk, glib};

/// Icon bytes for a hook-reported agent kind; `omp` shares Pi's mark and `campfire` has none,
/// like upstream's `CmuxTaskManagerCodingAgentDefinition.builtIns`.
fn icon_bytes(kind: &str) -> Option<&'static [u8]> {
    macro_rules! icon {
        ($name:literal) => {
            include_bytes!(concat!("../resources/agent-icons/", $name, ".png")).as_slice()
        };
    }
    Some(match kind {
        "claude" => icon!("claude"),
        "codex" => icon!("codex"),
        "grok" => icon!("grok"),
        "gemini" => icon!("gemini"),
        "kiro" => icon!("kiro"),
        "antigravity" => icon!("antigravity"),
        "hermes-agent" => icon!("hermes-agent"),
        "kimi" => icon!("kimi"),
        "copilot" => icon!("copilot"),
        "codebuddy" => icon!("codebuddy"),
        "factory" => icon!("factory"),
        "qoder" => icon!("qoder"),
        "opencode" => icon!("opencode"),
        "cursor" => icon!("cursor"),
        "pi" | "omp" => icon!("pi"),
        "amp" => icon!("amp"),
        "rovodev" => icon!("rovodev"),
        _ => return None,
    })
}

/// The mark for an agent kind, or None for unknown kinds (and non-agent bindings like local-tmux).
pub fn texture(kind: &str) -> Option<gdk::Texture> {
    let bytes = glib::Bytes::from_static(icon_bytes(kind)?);
    gdk::Texture::from_bytes(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use gtk4::prelude::*;

    /// Every agent the hooks can record resumes for has a mark decision, and each mark decodes.
    #[test]
    fn every_resumable_agent_has_a_decodable_mark() {
        for agent in crate::agent_resume::AGENTS {
            match super::icon_bytes(agent.kind) {
                Some(_) => {
                    let texture = super::texture(agent.kind)
                        .unwrap_or_else(|| panic!("{} icon does not decode", agent.kind));
                    assert_eq!((texture.width(), texture.height()), (42, 42), "{}", agent.kind);
                }
                None => assert_eq!(agent.kind, "campfire", "{} has no icon", agent.kind),
            }
        }
        assert!(super::icon_bytes(crate::local_tmux::RESUME_KIND).is_none());
    }
}
