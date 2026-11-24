//! What a screen reader is told, as a page's elements: each [`AccessNode`] as a hidden element
//! with its ARIA role, name, states and place, nested as the nodes are. Only what changed since
//! the last run is sent: a node's record when it differs, the order when a node came, went or
//! moved. The loader (`web.js`) keeps the elements; this says what they are, as JSON.
//!
//! The elements lie where their parts are drawn, see-through and ignoring the pointer, so a
//! reader that explores by touch finds them where they show.
use crate::accessibility::{AccessNode, Role};
use crate::id::{ById, Id};
use std::fmt::Write;

/// What was last sent: each node's record (by a hash of it), the order of all, the focused
/// node, and whether anything was sent yet.
#[derive(Default)]
pub(crate) struct Mirror {
    sent: ById<u64>,
    order: u64,
    focus: Option<Id>,
    started: bool,
}

/// The ARIA role a node's element takes ("" for plain text), and whether the element can take
/// the keyboard.
pub(crate) fn role_of(node: &AccessNode) -> &'static str {
    match (node.role, node.on) {
        (Role::Text, _) => "",
        (Role::Button | Role::Fold, _) => "button",
        (Role::CheckBox, _) => "checkbox",
        (Role::Switch, _) => "switch",
        (Role::RadioButton, _) => "radio",
        (Role::Tab, _) => "tab",
        (Role::Row, _) => "option",
        (Role::Slider, _) => "slider",
        (Role::SpinButton, _) => "spinbutton",
        (Role::TextBox | Role::PasswordBox, _) => "textbox",
        (Role::ComboBox, _) => "combobox",
        (Role::MenuItem, None) | (Role::Submenu, _) => "menuitem",
        (Role::MenuItem, Some(_)) => "menuitemcheckbox",
        (Role::MenuItemRadio, _) => "menuitemradio",
        (Role::ColumnHeader, _) => "columnheader",
        (Role::Progress, _) => "progressbar",
        (Role::Group, _) => "group",
        (Role::Dialog, _) => "dialog",
        (Role::Menu, _) => "menu",
        (Role::MenuBar, _) => "menubar",
        (Role::Tabs, _) => "tablist",
        (Role::Link, _) => "link",
        (Role::Table, _) => "grid",
        (Role::TableRow, _) => "row",
        (Role::Cell, _) => "gridcell",
        (Role::Document, _) => "document",
        (Role::Heading, _) => "heading",
        (Role::Paragraph, _) => "paragraph",
        (Role::List, _) => "list",
        (Role::ListItem, _) => "listitem",
        (Role::Code, _) => "code",
        (Role::Quote, _) => "blockquote",
        (Role::ToolBar, _) => "toolbar",
        (Role::Tree, _) => "tree",
        (Role::TreeItem, _) => "treeitem",
        (Role::Picture, _) => "img",
    }
}

/// A node's ARIA attributes beyond its role and name: its states, value, place in its set,
/// shortcut.
pub(crate) fn states_of(node: &AccessNode) -> Vec<(&'static str, String)> {
    let mut said = Vec::new();
    let mut say = |name: &'static str, value: String| said.push((name, value));
    let on = node.on == Some(true);
    match node.role {
        Role::CheckBox | Role::Switch | Role::RadioButton | Role::MenuItemRadio => say("aria-checked", on.to_string()),
        Role::MenuItem if node.on.is_some() => say("aria-checked", on.to_string()),
        Role::Button if node.on.is_some() => say("aria-pressed", on.to_string()),
        Role::Tab | Role::Row => say("aria-selected", on.to_string()),
        Role::TableRow | Role::TreeItem if node.on.is_some() => say("aria-selected", on.to_string()),
        Role::Fold | Role::ComboBox | Role::Submenu => say("aria-expanded", on.to_string()),
        Role::Tree if on => say("aria-multiselectable", "true".into()),
        Role::Dialog => say("aria-modal", "true".into()),
        Role::TextBox | Role::PasswordBox if node.on.is_some() => say("aria-expanded", on.to_string()),
        _ => {}
    }
    if node.role == Role::Submenu {
        say("aria-haspopup", "menu".into());
    }
    if let Some(open) = node.expanded {
        say("aria-expanded", open.to_string());
    }
    if node.level > 0 {
        say("aria-level", node.level.to_string());
    }
    if let Some([value, least, most]) = node.value_range {
        say("aria-valuenow", value.to_string());
        say("aria-valuemin", least.to_string());
        say("aria-valuemax", most.to_string());
    }
    if let Some([nth, count]) = node.position {
        say("aria-posinset", nth.to_string());
        say("aria-setsize", count.to_string());
    }
    match (node.role, node.grid) {
        (Role::Table, Some([rows, columns])) => {
            say("aria-rowcount", rows.to_string());
            say("aria-colcount", columns.to_string());
        }
        (Role::Cell, Some([row, column])) => {
            say("aria-rowindex", (row + 1).to_string());
            say("aria-colindex", (column + 1).to_string());
        }
        _ => {}
    }
    if let Some(descending) = node.sorted {
        say("aria-sort", if descending { "descending" } else { "ascending" }.into());
    }
    if !node.shortcut.is_empty() {
        say("aria-keyshortcuts", node.shortcut.replace("Ctrl", "Control").replace(' ', ""));
    }
    if !node.enabled {
        say("aria-disabled", "true".into());
    }
    if matches!(node.role, Role::TextBox | Role::PasswordBox) && !node.misspelled.is_empty() {
        say("aria-invalid", "spelling".into());
    }
    said
}

/// A node as the loader takes it: its key, role, name, the text it shows, attributes, place,
/// and whether it takes the keyboard.
fn record(node: &AccessNode) -> String {
    let mut out = String::with_capacity(96 + node.name.len());
    let _ = write!(out, "{{\"k\":\"{:x}\",\"r\":\"{}\",\"n\":", node.id, role_of(node));
    // Plain text shows its name; a text box its text (a password box its dots).
    let shown = match node.role {
        Role::Text | Role::Paragraph | Role::Heading | Role::Code => node.name.as_str(),
        Role::TextBox | Role::PasswordBox | Role::ComboBox => node.value.as_str(),
        _ => "",
    };
    quote(&mut out, if node.role == Role::Text { "" } else { &node.name });
    out.push_str(",\"t\":");
    quote(&mut out, shown);
    out.push_str(",\"a\":[");
    for (at, (name, value)) in states_of(node).into_iter().enumerate() {
        out.push_str(if at == 0 { "[" } else { ",[" });
        quote(&mut out, name);
        out.push(',');
        quote(&mut out, &value);
        out.push(']');
    }
    let r = node.rect;
    let _ = write!(out, "],\"x\":{:.1},\"y\":{:.1},\"w\":{:.1},\"h\":{:.1},\"f\":{}}}", r.x, r.y, r.w.max(0.0), r.h.max(0.0), node.takes_keyboard() as u8);
    out
}

/// `text` as a JSON string.
fn quote(out: &mut String, text: &str) {
    out.push('"');
    for letter in text.chars() {
        match letter {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            letter if (letter as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", letter as u32);
            }
            letter => out.push(letter),
        }
    }
    out.push('"');
}

/// A plain hash: FNV-1a over bytes.
fn hash(bytes: &[u8], from: u64) -> u64 {
    bytes.iter().fold(from, |sum, byte| (sum ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3))
}

const FNV: u64 = 0xcbf2_9ce4_8422_2325;

impl Mirror {
    /// What the loader needs to bring its elements in line with `nodes` (in order, each group
    /// before its content), the node `focused` (if any), and what was `announced`: as JSON, or
    /// `None` when nothing changed.
    pub(crate) fn update(&mut self, nodes: &[AccessNode], announced: &[String]) -> Option<String> {
        let mut order = FNV;
        for node in nodes {
            let parent = node.parent.and_then(|at| nodes.get(at)).map_or(0, |parent| parent.id);
            order = hash(&node.id.to_le_bytes(), hash(&parent.to_le_bytes(), order));
        }
        let mut changed = Vec::new();
        let mut kept: ById<u64> = ById::default();
        for node in nodes {
            let said = record(node);
            let sum = hash(said.as_bytes(), FNV);
            if self.sent.get(&node.id) != Some(&sum) {
                changed.push(said);
            }
            kept.insert(node.id, sum);
        }
        let reordered = order != self.order || !self.started;
        let focus = nodes.iter().find(|node| node.focused).map(|node| node.id);
        if !reordered && changed.is_empty() && announced.is_empty() && focus == self.focus {
            return None;
        }
        (self.sent, self.order, self.focus, self.started) = (kept, order, focus, true);
        let mut out = String::from("{\"nodes\":[");
        out.push_str(&changed.join(","));
        out.push(']');
        if reordered {
            out.push_str(",\"order\":[");
            for (at, node) in nodes.iter().enumerate() {
                let parent = node.parent.and_then(|at| nodes.get(at)).map_or(String::new(), |parent| format!("{:x}", parent.id));
                let _ = write!(out, "{}[\"{:x}\",\"{}\"]", if at == 0 { "" } else { "," }, node.id, parent);
            }
            out.push(']');
        }
        let focused = focus.map_or(String::new(), |id| format!("{id:x}"));
        let _ = write!(out, ",\"focus\":\"{focused}\",\"say\":[");
        for (at, said) in announced.iter().enumerate() {
            if at > 0 {
                out.push(',');
            }
            quote(&mut out, said);
        }
        out.push_str("]}");
        Some(out)
    }

    /// Forgets what was sent: the next update sends everything.
    pub(crate) fn forget(&mut self) {
        *self = Mirror::default();
    }
}

/// The node a key names (as `update` writes it), if any.
pub(crate) fn id_of(key: &str) -> Option<Id> {
    Id::from_str_radix(key, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Trial;
    use crate::ui::Ui;

    /// A small window: a heading, a tick box, a button, a slider and a text box.
    struct Form {
        ticked: bool,
        amount: f32,
        name: String,
    }

    fn form(form: &mut Form, ui: &mut Ui) {
        ui.column().pad(10.0).gap(8.0).open();
        ui.label("Settings");
        ui.checkbox(&mut form.ticked, "Keep \"this\"");
        ui.button("Save");
        ui.slider(&mut form.amount, 0.0..=10.0);
        ui.text_box(&mut form.name, "Name");
        ui.close();
    }

    /// A JSON value's text after `"name":` in `json`, as far as its closing quote.
    fn after<'a>(json: &'a str, name: &str) -> &'a str {
        let from = json.find(&format!("\"{name}\":")).unwrap() + name.len() + 3;
        &json[from..]
    }

    #[test]
    fn readers_hear_roles_names_and_states_as_aria_has_them() {
        let mut trial = Trial::new(Form { ticked: true, amount: 4.0, name: "Ann".into() }, form);
        trial.ui.set_screen_reader(true);
        trial.run();
        trial.run();
        let nodes = trial.ui.access_nodes().to_vec();
        let find = |role: Role| nodes.iter().find(|node| node.role == role).unwrap();
        assert_eq!(role_of(find(Role::CheckBox)), "checkbox");
        assert_eq!(states_of(find(Role::CheckBox)), [("aria-checked", "true".to_string())]);
        assert_eq!(role_of(find(Role::Slider)), "slider");
        let slider = states_of(find(Role::Slider));
        assert!(slider.contains(&("aria-valuenow", "4".into())) && slider.contains(&("aria-valuemax", "10".into())), "{slider:?}");
        assert_eq!(role_of(find(Role::Text)), "");
        let mut mirror = Mirror::default();
        let first = mirror.update(&nodes, &[]).unwrap();
        // Every node once, in order, with its parent; quotes in names escaped.
        assert_eq!(first.matches("\"k\":").count(), nodes.len());
        assert!(first.contains("\"order\":[") && first.contains("Keep \\\"this\\\""), "{first}");
        assert!(first.contains("\"r\":\"textbox\"") && first.contains("\"t\":\"Ann\""), "{first}");
        assert!(first.contains("\"t\":\"Settings\""), "{first}");
        // Nothing changed: nothing sent.
        assert_eq!(mirror.update(&nodes, &[]), None);
        // An announcement alone.
        let said = mirror.update(&nodes, &["Saved".into()]).unwrap();
        assert!(said.ends_with("\"say\":[\"Saved\"]}") && after(&said, "nodes").starts_with("[]"), "{said}");
    }

    #[test]
    fn only_what_changed_is_sent_again() {
        let mut trial = Trial::new(Form { ticked: false, amount: 4.0, name: String::new() }, form);
        trial.ui.set_screen_reader(true);
        trial.run();
        let mut mirror = Mirror::default();
        mirror.update(trial.ui.access_nodes(), &[]).unwrap();
        // The tick box is ticked: its record alone, and no new order.
        trial.app.ticked = true;
        trial.ui.event(crate::event::Event::Focused(true));
        trial.run();
        let next = mirror.update(trial.ui.access_nodes(), &[]).unwrap();
        assert_eq!(next.matches("\"k\":").count(), 1, "{next}");
        assert!(next.contains("\"aria-checked\",\"true\"") && !next.contains("\"order\""), "{next}");
        // A node gone: a new order without it.
        let before = trial.ui.access_nodes().len();
        let nodes: Vec<AccessNode> = trial.ui.access_nodes().iter().filter(|node| node.role != Role::Button).cloned().collect();
        let fewer = mirror.update(&nodes, &[]).unwrap();
        assert!(fewer.contains("\"order\"") && fewer.matches("],[").count() + 1 == before - 1, "{fewer}");
        // The key of a node reads back as its id; forgetting sends all again.
        assert_eq!(id_of(&format!("{:x}", nodes[1].id)), Some(nodes[1].id));
        mirror.forget();
        assert_eq!(mirror.update(&nodes, &[]).unwrap().matches("\"k\":").count(), nodes.len());
    }

    #[test]
    fn the_focused_node_is_named() {
        let mut trial = Trial::new(Form { ticked: false, amount: 1.0, name: String::new() }, form);
        trial.ui.set_screen_reader(true);
        trial.run();
        let mut mirror = Mirror::default();
        let before = mirror.update(trial.ui.access_nodes(), &[]).unwrap();
        assert!(before.contains("\"focus\":\"\""), "{before}");
        // Focus moves, and nothing else changes: that alone is sent.
        trial.ui.event(crate::event::Event::Key(crate::event::Key::Tab, crate::event::Mods::NONE));
        trial.run();
        let nodes = trial.ui.access_nodes().to_vec();
        let focused = nodes.iter().find(|node| node.focused).expect("a focused node");
        let sent = mirror.update(&nodes, &[]).unwrap();
        assert!(sent.contains(&format!("\"focus\":\"{:x}\"", focused.id)), "{sent}");
        assert_eq!(mirror.update(&nodes, &[]), None);
    }
}
