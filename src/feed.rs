//! Feed (upstream `docs/feed.md`): permission requests, questions and plans that agent hooks
//! park here until a human answers from the right panel. A hook waits at most two minutes;
//! without a decision the agent falls back to its own terminal prompt.
use crate::app_state::AppStateRef;
use gtk4::{gio, glib, prelude::*};
use serde_json::{json, Value};
use uuid::Uuid;

/// Upstream's soft wait: the hook never blocks its agent longer than this.
pub const WAIT_SECONDS: u32 = 120;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Permission,
    Question,
    Plan,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Permission => "permission",
            Kind::Question => "question",
            Kind::Plan => "plan",
        }
    }
}

/// One parked hook: what it asks and the socket response that wakes it.
pub struct Item {
    pub id: Uuid,
    pub kind: Kind,
    pub source: String,
    pub tool: String,
    /// The agent's tool input: questions, plan text or the tool call being approved.
    pub detail: Value,
    pub surface: Option<Uuid>,
    pub session: Option<String>,
    req_id: Value,
    resp_tx: crate::socket::commands::RespTx,
}

#[derive(Default)]
pub struct Feed {
    items: Vec<Item>,
}

/// Right-side panel and the header badge counting pending items.
pub struct Panel {
    pub revealer: gtk4::Revealer,
    list: gtk4::Box,
    badge: gtk4::Label,
}

/// Validated `feed.push` parameters, without the response channel.
#[derive(Debug)]
pub struct Request {
    pub kind: Kind,
    pub source: String,
    pub tool: String,
    pub detail: Value,
    pub surface: Option<Uuid>,
    pub session: Option<String>,
}

pub fn parse(params: &Value) -> Result<Request, &'static str> {
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| value.len() <= 1024 && !value.chars().any(char::is_control))
    };
    let kind = match text("kind") {
        Some("permission") => Kind::Permission,
        Some("question") => Kind::Question,
        Some("plan") => Kind::Plan,
        _ => return Err("kind must be permission, question or plan"),
    };
    let detail = params.get("detail").cloned().unwrap_or(json!({}));
    if !detail.is_object() {
        return Err("detail must be an object");
    }
    let surface = match params.get("surface_id") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or("surface_id must be a UUID")?,
        ),
    };
    Ok(Request {
        kind,
        source: text("source").ok_or("source is required")?.to_owned(),
        tool: text("tool_name").unwrap_or_default().to_owned(),
        detail,
        surface,
        session: text("session_id").map(ToOwned::to_owned),
    })
}

fn workspace_of(state: &crate::app_state::AppState, surface: Option<Uuid>) -> Option<usize> {
    let surface = surface?.to_string();
    state
        .split_engines
        .iter()
        .position(|engine| engine.find_pane_id_by_uuid(&surface).is_some())
}

fn publish(state: &crate::app_state::AppState, name: &str, item: &Item, extra: Value) {
    let mut payload = json!({
        "request_id": item.id, "kind": item.kind.name(), "_source": item.source,
        "tool_name": item.tool, "session_id": item.session,
    });
    if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        payload.extend(extra.clone());
    }
    crate::events::publish(
        name,
        &item.source,
        crate::events::Scope {
            workspace: workspace_of(state, item.surface).map(|index| state.workspaces[index].uuid),
            surface: item.surface.map(|id| id.to_string()),
            ..Default::default()
        },
        payload,
    );
}

/// Park a hook request, show it and bound its wait. The response is sent by [`resolve`].
pub fn push(
    state: &AppStateRef,
    req_id: Value,
    request: Request,
    resp_tx: crate::socket::commands::RespTx,
) {
    let id = Uuid::new_v4();
    {
        let mut s = state.borrow_mut();
        let item = Item {
            id,
            kind: request.kind,
            source: request.source,
            tool: request.tool,
            detail: request.detail,
            surface: request.surface,
            session: request.session,
            req_id,
            resp_tx,
        };
        publish(&s, "feed.item.received", &item, json!({}));
        let first = s.feed.items.is_empty();
        s.feed.items.push(item);
        if first {
            watch_abandoned(state);
        }
        if let Some(panel) = &s.feed_panel {
            panel.revealer.set_reveal_child(true);
        }
    }
    refresh(state);
    let weak = std::rc::Rc::downgrade(state);
    glib::timeout_add_seconds_local_once(WAIT_SECONDS, move || {
        if let Some(state) = weak.upgrade() {
            resolve(&state, id, None, "timeout");
        }
    });
}

/// Drop items whose hook went away (agent interrupted, hook killed) while any are pending.
fn watch_abandoned(state: &AppStateRef) {
    let weak = std::rc::Rc::downgrade(state);
    glib::timeout_add_seconds_local(1, move || {
        let Some(state) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let abandoned: Vec<Uuid> = state
            .borrow()
            .feed
            .items
            .iter()
            .filter(|item| item.resp_tx.is_closed())
            .map(|item| item.id)
            .collect();
        for id in abandoned {
            resolve(&state, id, None, "abandoned");
        }
        if state.borrow().feed.items.is_empty() {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// Wake the hook with `decision` (None: let the agent ask in its terminal).
pub fn resolve(state: &AppStateRef, id: Uuid, decision: Option<Value>, outcome: &str) {
    {
        let mut s = state.borrow_mut();
        let Some(index) = s.feed.items.iter().position(|item| item.id == id) else {
            return;
        };
        let item = s.feed.items.remove(index);
        let result = json!({"decision": decision});
        if decision.is_some() {
            publish(&s, "feed.item.resolved", &item, json!({"choice": decision.as_ref().and_then(|d| d.get("choice")).cloned()}));
        }
        publish(&s, "feed.item.completed", &item, json!({"result": outcome}));
        let _ = item
            .resp_tx
            .send(json!({"id": item.req_id, "ok": true, "result": result}));
    }
    refresh(state);
}

/// Show or hide the panel.
pub fn toggle(state: &AppStateRef) {
    if let Some(panel) = &state.borrow().feed_panel {
        panel.revealer.set_reveal_child(!panel.revealer.reveals_child());
    }
}

/// Header button bound to `action`, with a count badge overlaid on its corner.
pub fn header_button(
    icons: &[&str],
    tooltip: &str,
    action: &str,
) -> (gtk4::Overlay, gtk4::Label) {
    let button = gtk4::Button::new();
    button.set_child(Some(&gtk4::Image::from_gicon(&gio::ThemedIcon::from_names(icons))));
    button.set_tooltip_text(Some(tooltip));
    button.set_action_name(Some(action));
    button.add_css_class("headerbar-btn");
    let badge = gtk4::Label::new(None);
    badge.add_css_class("header-badge");
    badge.set_halign(gtk4::Align::End);
    badge.set_valign(gtk4::Align::Start);
    badge.set_can_target(false);
    badge.set_visible(false);
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&button));
    overlay.add_overlay(&badge);
    (overlay, badge)
}

/// Show `count` on a header badge; hidden at zero.
pub fn set_badge(badge: &gtk4::Label, count: usize) {
    badge.set_text(&if count > 99 { "99+".into() } else { count.to_string() });
    badge.set_visible(count > 0);
}

/// Build the (initially hidden) right panel; `badge` is the header button's counter.
pub fn build_panel(badge: gtk4::Label) -> Panel {
    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    column.set_width_request(340);
    column.add_css_class("feed-panel");
    let title = gtk4::Label::new(None);
    title.set_markup("<b>Feed</b>");
    title.set_xalign(0.0);
    title.set_margin_start(12);
    title.set_margin_top(8);
    column.append(&title);
    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    list.set_margin_start(8);
    list.set_margin_end(8);
    list.set_margin_bottom(8);
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    column.append(&scroll);
    let revealer = gtk4::Revealer::builder()
        .transition_type(gtk4::RevealerTransitionType::SlideLeft)
        .child(&column)
        .reveal_child(false)
        .build();
    Panel {
        revealer,
        list,
        badge,
    }
}

/// Rebuild the cards from the pending items.
pub fn refresh(state: &AppStateRef) {
    let s = state.borrow();
    let Some(panel) = &s.feed_panel else {
        return;
    };
    set_badge(&panel.badge, s.feed.items.len());
    while let Some(child) = panel.list.first_child() {
        panel.list.remove(&child);
    }
    if s.feed.items.is_empty() {
        let empty = gtk4::Label::new(Some("No agent is waiting for you."));
        empty.add_css_class("dim-label");
        empty.set_margin_top(24);
        panel.list.append(&empty);
        return;
    }
    for item in &s.feed.items {
        let workspace = workspace_of(&s, item.surface).map(|index| s.workspaces[index].name.clone());
        panel.list.append(&card(state, item, workspace));
    }
}

fn wrapped(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.set_wrap(true);
    label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label
}

/// One-line description of a tool call for a permission card.
pub fn summary(tool: &str, input: &Value) -> String {
    let field = ["command", "file_path", "path", "url", "pattern", "description"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str));
    let text = field.map(ToOwned::to_owned).unwrap_or_else(|| input.to_string());
    let mut text: String = text.chars().take(400).collect();
    if text.chars().count() == 400 {
        text.push('…');
    }
    format!("{tool}: {text}")
}

fn decision_button(
    state: &AppStateRef,
    id: Uuid,
    label: &str,
    decision: Option<Value>,
) -> gtk4::Button {
    let button = gtk4::Button::with_label(label);
    let weak = std::rc::Rc::downgrade(state);
    button.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            let outcome = if decision.is_some() { "decided" } else { "terminal" };
            resolve(&state, id, decision.clone(), outcome);
        }
    });
    button
}

fn card(state: &AppStateRef, item: &Item, workspace: Option<String>) -> gtk4::Frame {
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    let source = match item.source.as_str() {
        "claude" => "Claude".to_owned(),
        other => other.to_owned(),
    };
    let heading = gtk4::Label::new(None);
    heading.set_markup(&glib::markup_escape_text(&match workspace {
        Some(workspace) => format!("{source} · {workspace}"),
        None => source,
    }));
    heading.add_css_class("heading");
    heading.set_xalign(0.0);
    body.append(&heading);
    let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    match item.kind {
        Kind::Permission => {
            body.append(&wrapped(&summary(&item.tool, &item.detail)));
            for (label, choice) in [("Allow Once", "once"), ("Always Allow", "always"), ("Deny", "deny")] {
                actions.append(&decision_button(state, item.id, label, Some(json!({"choice": choice}))));
            }
        }
        Kind::Plan => {
            let plan = wrapped(item.detail.get("plan").and_then(Value::as_str).unwrap_or(""));
            let scroll = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .max_content_height(240)
                .propagate_natural_height(true)
                .child(&plan)
                .build();
            body.append(&scroll);
            for (label, choice) in [("Manual", "manual"), ("Auto", "auto"), ("Deny", "deny")] {
                actions.append(&decision_button(state, item.id, label, Some(json!({"choice": choice}))));
            }
        }
        Kind::Question => {
            let mut answers = Vec::new();
            for question in item.detail["questions"].as_array().into_iter().flatten() {
                let text = question["question"].as_str().unwrap_or_default().to_owned();
                body.append(&wrapped(&text));
                let multiple = question["multiSelect"].as_bool().unwrap_or(false);
                let mut group: Option<gtk4::CheckButton> = None;
                let mut options = Vec::new();
                for option in question["options"].as_array().into_iter().flatten() {
                    let label = option["label"].as_str().unwrap_or_default().to_owned();
                    let check = gtk4::CheckButton::with_label(&label);
                    if let Some(description) = option["description"].as_str() {
                        check.set_tooltip_text(Some(description));
                    }
                    if !multiple {
                        match &group {
                            Some(first) => check.set_group(Some(first)),
                            None => group = Some(check.clone()),
                        }
                    }
                    body.append(&check);
                    options.push((label, check));
                }
                answers.push((text, options));
            }
            let submit = gtk4::Button::with_label("Submit");
            let weak = std::rc::Rc::downgrade(state);
            let id = item.id;
            submit.connect_clicked(move |_| {
                let chosen: serde_json::Map<String, Value> = answers
                    .iter()
                    .filter_map(|(question, options)| {
                        let picked: Vec<&str> = options
                            .iter()
                            .filter(|(_, check)| check.is_active())
                            .map(|(label, _)| label.as_str())
                            .collect();
                        (!picked.is_empty()).then(|| (question.clone(), json!(picked.join(", "))))
                    })
                    .collect();
                if let Some(state) = weak.upgrade() {
                    let decision = json!({"choice": "submit", "answers": chosen});
                    resolve(&state, id, Some(decision), "decided");
                }
            });
            actions.append(&submit);
        }
    }
    let terminal = decision_button(state, item.id, "In Terminal", None);
    terminal.add_css_class("flat");
    terminal.set_tooltip_text(Some("Let the agent ask in its own terminal"));
    actions.append(&terminal);
    body.append(&actions);
    let frame = gtk4::Frame::new(None);
    frame.set_child(Some(&body));
    if let Some(surface) = item.surface {
        frame.set_tooltip_text(Some("Double-click to jump to the agent"));
        let click = gtk4::GestureClick::new();
        let weak = std::rc::Rc::downgrade(state);
        click.connect_pressed(move |_, presses, _, _| {
            if presses == 2 {
                if let Some(state) = weak.upgrade() {
                    focus(&state, surface);
                }
            }
        });
        frame.add_controller(click);
    }
    frame
}

/// Select the agent's workspace and focus its terminal.
fn focus(state: &AppStateRef, surface: Uuid) {
    let mut s = state.borrow_mut();
    let Some(index) = workspace_of(&s, Some(surface)) else {
        return;
    };
    s.switch_to_index(index);
    s.split_engines[index].focus_surface(&surface.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_validates_kind_and_surface() {
        let request = parse(&json!({"kind": "question", "source": "claude",
            "tool_name": "AskUserQuestion", "detail": {"questions": []},
            "surface_id": "35af159f-6832-4132-a4df-d5a2aaaf9608"}))
        .unwrap();
        assert_eq!(request.kind, Kind::Question);
        assert!(request.surface.is_some());
        assert!(parse(&json!({"kind": "other", "source": "claude"})).is_err());
        assert!(parse(&json!({"kind": "plan", "source": "claude", "surface_id": "x"})).is_err());
        assert!(parse(&json!({"kind": "plan", "source": "claude", "detail": []})).is_err());
    }

    #[test]
    fn summary_prefers_the_meaningful_field() {
        assert_eq!(summary("Bash", &json!({"command": "ls", "description": "list"})), "Bash: ls");
        assert!(summary("Edit", &json!({"file_path": "/a"})).ends_with("/a"));
        assert!(summary("X", &json!({"blob": "y".repeat(900)})).ends_with('…'));
    }
}
