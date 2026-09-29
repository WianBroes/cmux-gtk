//! Rewrite upstream `browser` command lines into the form clap expects.
//!
//! Upstream invokes `cmux browser --surface <h> <verb> …` or `cmux browser <h> <verb> …`,
//! while this CLI parses `cmux browser <verb> <surface> …`. The rewrite runs on the raw
//! argument vector before clap sees it, so both upstream spellings parse as ours.

/// Verbs that carry the surface as a `--surface` flag placed right after the verb.
const FLAG_VERBS: &[&str] = &[
    "open",
    "list",
    "close",
    "identify",
    "stream-enable",
    "stream-disable",
    "help",
];

/// Verbs followed by a second positional (the action to run) before the surface.
const GROUP_VERBS: &[&str] = &["get", "is", "dialog", "state"];

/// Every verb this CLI parses, used to tell a bare handle from a leading verb.
const KNOWN_VERBS: &[&str] = &[
    "open",
    "list",
    "close",
    "identify",
    "snapshot",
    "click",
    "dblclick",
    "fill",
    "type",
    "press",
    "key",
    "keydown",
    "keyup",
    "hover",
    "focus",
    "check",
    "uncheck",
    "scroll",
    "select",
    "eval",
    "wait",
    "goto",
    "navigate",
    "back",
    "forward",
    "reload",
    "get",
    "get-url",
    "url",
    "get-title",
    "get-text",
    "get-html",
    "is",
    "frame",
    "dialog",
    "console",
    "errors",
    "highlight",
    "state",
    "screenshot",
    "viewport",
    "cookies",
    "storage",
    "addinitscript",
    "addstyle",
    "addscript",
    "download",
    "download-wait",
    "stream-enable",
    "stream-disable",
    "help",
];

/// Rewrite upstream browser forms (`browser --surface S verb …`, `browser S verb …`)
/// into the clap form (`browser verb S …`); any other command line is returned unchanged.
pub fn normalize(args: Vec<String>) -> Vec<String> {
    let Some(index) = (1..args.len()).find(|position| args[*position] == "browser") else {
        return args;
    };
    let mut rest = args[index + 1..].to_vec();
    let Some(surface) = take_surface(&mut rest) else {
        return args;
    };
    if rest.is_empty() {
        rest = vec!["--surface".to_string(), surface];
    } else if FLAG_VERBS.contains(&rest[0].as_str()) {
        rest.splice(1..1, ["--surface".to_string(), surface]);
    } else if GROUP_VERBS.contains(&rest[0].as_str())
        && rest
            .get(1)
            .is_some_and(|argument| !argument.starts_with('-'))
    {
        rest.splice(2..2, [surface]);
    } else {
        rest.splice(1..1, [surface]);
    }
    let mut rewritten = args;
    rewritten.splice(index + 1.., rest);
    rewritten
}

/// Remove the first `--surface X` or `--surface=X` from `rest`, else a bare leading handle
/// that is neither a flag nor a known verb; `None` leaves `rest` untouched.
fn take_surface(rest: &mut Vec<String>) -> Option<String> {
    let mut position = 0;
    while position < rest.len() {
        if rest[position] == "--surface" {
            if position + 1 < rest.len() {
                let surface = rest.remove(position + 1);
                rest.remove(position);
                return Some(surface);
            }
        } else if let Some(surface) = rest[position].strip_prefix("--surface=") {
            let surface = surface.to_string();
            rest.remove(position);
            return Some(surface);
        }
        position += 1;
    }
    let first = rest.first()?;
    if first.starts_with('-') || KNOWN_VERBS.contains(&first.as_str()) {
        return None;
    }
    Some(rest.remove(0))
}

#[cfg(test)]
mod tests {
    use super::normalize;

    /// Normalize a full invocation, program name included.
    fn rewritten(arguments: &[&str]) -> Vec<String> {
        normalize(
            arguments
                .iter()
                .map(|argument| (*argument).to_string())
                .collect(),
        )
    }

    /// A bare upstream handle moves behind its verb.
    #[test]
    fn bare_handle_moves_after_the_verb() {
        assert_eq!(
            rewritten(&["cmux", "browser", "surface:2", "click", "#b"]),
            ["cmux", "browser", "click", "surface:2", "#b"]
        );
    }

    /// A grouped verb keeps its action in front of the surface.
    #[test]
    fn flag_surface_lands_inside_a_grouped_verb() {
        assert_eq!(
            rewritten(&[
                "cmux",
                "browser",
                "--surface",
                "surface:2",
                "get",
                "attr",
                "#i",
                "--attr",
                "href"
            ]),
            [
                "cmux",
                "browser",
                "get",
                "attr",
                "surface:2",
                "#i",
                "--attr",
                "href"
            ]
        );
        assert_eq!(
            rewritten(&[
                "cmux",
                "browser",
                "--surface=surface:2",
                "is",
                "visible",
                "#t"
            ]),
            ["cmux", "browser", "is", "visible", "surface:2", "#t"]
        );
    }

    /// A simple verb takes the surface right after its own name.
    #[test]
    fn bare_handle_lands_after_a_simple_verb() {
        assert_eq!(
            rewritten(&["cmux", "browser", "surface:2", "console", "clear"]),
            ["cmux", "browser", "console", "surface:2", "clear"]
        );
    }

    /// `identify` takes the surface as its own `--surface` flag.
    #[test]
    fn identify_keeps_the_surface_flag() {
        assert_eq!(
            rewritten(&["cmux", "browser", "--surface", "surface:2", "identify"]),
            ["cmux", "browser", "identify", "--surface", "surface:2"]
        );
    }

    /// Global options in front of `browser` stay where they are.
    #[test]
    fn leading_global_options_stay_in_front() {
        assert_eq!(
            rewritten(&["cmux", "--json", "browser", "surface:2", "url"]),
            ["cmux", "--json", "browser", "url", "surface:2"]
        );
    }

    /// The clap spelling and command lines without `browser` are returned untouched.
    #[test]
    fn clap_spelling_and_other_commands_are_untouched() {
        let clap_line = ["cmux", "browser", "click", "surface:2", "#b"];
        assert_eq!(rewritten(&clap_line), clap_line);
        let other_line = ["cmux", "list-surfaces", "--workspace", "workspace:2"];
        assert_eq!(rewritten(&other_line), other_line);
    }

    /// The new verbs keep the clap spelling and move a bare upstream handle behind them.
    #[test]
    fn advanced_verbs_keep_their_clap_spelling() {
        let viewport = ["cmux", "browser", "viewport", "surface:2", "800", "600"];
        assert_eq!(rewritten(&viewport), viewport);
        let cookies = ["cmux", "browser", "cookies", "surface:2", "get"];
        assert_eq!(rewritten(&cookies), cookies);
        let storage = ["cmux", "browser", "storage", "surface:2", "local"];
        assert_eq!(rewritten(&storage), storage);
        assert_eq!(
            rewritten(&["cmux", "browser", "surface:2", "viewport", "800", "600"]),
            ["cmux", "browser", "viewport", "surface:2", "800", "600"]
        );
    }

    /// The script and download verbs keep the clap spelling and move a bare handle behind them.
    #[test]
    fn script_and_download_verbs_keep_their_clap_spelling() {
        for line in [
            ["cmux", "browser", "addinitscript", "surface:2", "window.x = 1;"].as_slice(),
            ["cmux", "browser", "addstyle", "surface:2", "body {}"].as_slice(),
            ["cmux", "browser", "addscript", "surface:2", "alert(1);"].as_slice(),
            ["cmux", "browser", "download", "surface:2", "#dl", "/tmp/f.zip"].as_slice(),
            [
                "cmux",
                "browser",
                "download-wait",
                "surface:2",
                "--timeout",
                "4000",
            ]
            .as_slice(),
        ] {
            assert_eq!(rewritten(line), line);
        }
        assert_eq!(
            rewritten(&["cmux", "browser", "surface:2", "addscript", "alert(1);"]),
            ["cmux", "browser", "addscript", "surface:2", "alert(1);"]
        );
        assert_eq!(
            rewritten(&["cmux", "browser", "surface:2", "download", "#dl", "/tmp/f.zip"]),
            ["cmux", "browser", "download", "surface:2", "#dl", "/tmp/f.zip"]
        );
        assert_eq!(
            rewritten(&[
                "cmux",
                "browser",
                "--surface",
                "surface:2",
                "download-wait",
                "--timeout",
                "4000"
            ]),
            [
                "cmux",
                "browser",
                "download-wait",
                "surface:2",
                "--timeout",
                "4000"
            ]
        );
    }
}
