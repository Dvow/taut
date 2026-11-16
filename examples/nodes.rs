//! A small node editor: numbers and words, sums of them, and a display, each a node on a board;
//! wired together, they compute. Right-clicking the board adds a node there (on a node, removes
//! it); the buttons above the board add one in the middle. Wire an output to an input by
//! dragging between them, or by keyboard: Enter on a port, arrows to the target port, Enter.
//!   cargo run -p taut --example nodes
#![windows_subsystem = "windows"]
use taut::geom::{Point, pt, rect};
use taut::{Did, Font, Graph, Node, Role, Ui, Window};

/// What a node does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Does {
    /// Holds a number the person sets.
    Number,
    /// Holds words the person types.
    Words,
    /// Adds two numbers, takes one from another, multiplies them, divides one by the other.
    Add,
    Subtract,
    Multiply,
    Divide,
    /// Shows a number large, under the words wired into it.
    Show,
}

/// Every kind of node, by the name it is added by.
const KINDS: [(Does, &str); 7] =
    [(Does::Number, "Number"), (Does::Words, "Words"), (Does::Add, "Add"), (Does::Subtract, "Subtract"), (Does::Multiply, "Multiply"), (Does::Divide, "Divide"), (Does::Show, "Show")];
/// What goes through the ports: numbers, and words. (Words do not go into a number.)
const NUMBER: &str = "number";
const WORDS: &str = "words";

/// What comes out of a node.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A number.
    Number(f64),
    /// Words.
    Words(String),
}

/// What a node of this editor holds: what it does, the number or the words the person gave
/// it (for the nodes that hold one), and what came out of it when the graph was last worked
/// out (nothing, where something it needs is not wired in, or it lies on a loop).
#[derive(Clone, Debug)]
pub struct Step {
    does: Does,
    number: f64,
    words: String,
    out: Option<Value>,
}

/// The editor: its graph, and where its menu was asked for (the node, and the point of the
/// board).
pub struct Sums {
    graph: Graph<Step>,
    menu: (Option<usize>, Point),
}

/// A node that does `does`, its corner at `at` on the board.
fn node(does: Does, at: Point) -> Node<Step> {
    let name = KINDS.iter().find(|(kind, _)| *kind == does).map_or("", |(_, name)| name);
    let node = Node::new(name, at.x, at.y, Step { does, number: 0.0, words: String::new(), out: None });
    match does {
        Does::Number => node.output("Value", NUMBER),
        Does::Words => node.output("Words", WORDS),
        Does::Show => node.input("Value", NUMBER).input("Caption", WORDS).width(180.0),
        _ => node.input("A", NUMBER).input("B", NUMBER).output("Result", NUMBER).width(180.0),
    }
}

/// Two numbers added and shown under a caption, and a node to multiply that is not wired yet.
impl Default for Sums {
    fn default() -> Self {
        let mut graph = Graph::new();
        graph.kinds = vec![NUMBER, WORDS];
        let mut first = node(Does::Number, pt(40.0, 40.0));
        first.data.number = 3.0;
        let mut second = node(Does::Number, pt(40.0, 190.0));
        second.data.number = 4.0;
        let mut caption = node(Does::Words, pt(300.0, 300.0));
        caption.data.words = "Total".into();
        for each in [first, second, node(Does::Add, pt(300.0, 90.0)), caption, node(Does::Show, pt(560.0, 140.0)), node(Does::Multiply, pt(560.0, 330.0))] {
            graph.add(each);
        }
        for (from, to) in [((0, 0), (2, 0)), ((1, 0), (2, 1)), ((2, 0), (4, 0)), ((3, 0), (4, 1))] {
            graph.wire(from, to);
        }
        work_out(&mut graph);
        Sums { graph, menu: (None, pt(0.0, 0.0)) }
    }
}

/// Works out what comes out of every node: each once what is wired into it is known, in
/// the order that makes so (a node on a loop never is).
fn work_out(graph: &mut Graph<Step>) {
    let mut waiting = vec![0; graph.nodes.len()];
    for wire in &graph.wires {
        waiting[wire.to.0] += 1;
    }
    let mut ready: Vec<usize> = (0..graph.nodes.len()).filter(|node| waiting[*node] == 0).collect();
    graph.nodes.iter_mut().for_each(|node| node.data.out = None);
    while let Some(node) = ready.pop() {
        graph.nodes[node].data.out = output(graph, node);
        for wire in graph.wires.iter().filter(|wire| wire.from.0 == node) {
            waiting[wire.to.0] -= 1;
            if waiting[wire.to.0] == 0 {
                ready.push(wire.to.0);
            }
        }
    }
}

/// What is wired into an input of a node, as it came out of the node it comes from.
fn input(graph: &Graph<Step>, node: usize, port: usize) -> Option<&Value> {
    graph.wire_into((node, port)).and_then(|wire| graph.nodes[wire.from.0].data.out.as_ref())
}

/// What comes out of a node, from what is wired into it.
fn output(graph: &Graph<Step>, node: usize) -> Option<Value> {
    let step = &graph.nodes[node].data;
    let number = |port: usize| match input(graph, node, port) {
        Some(Value::Number(number)) => Some(*number),
        _ => None,
    };
    let (a, b) = (number(0), number(1));
    let both = |work: fn(f64, f64) -> f64| a.zip(b).map(|(a, b)| Value::Number(work(a, b)));
    match step.does {
        Does::Number => Some(Value::Number(step.number)),
        Does::Words => Some(Value::Words(step.words.clone())),
        Does::Add => both(|a, b| a + b),
        Does::Subtract => both(|a, b| a - b),
        Does::Multiply => both(|a, b| a * b),
        Does::Divide => both(|a, b| a / b),
        Does::Show => a.map(Value::Number),
    }
}

/// A number as it is written: with no more figures after the point than it needs (six at
/// the most).
fn written(number: f64) -> String {
    if !number.is_finite() {
        return "Not a number".into();
    }
    let fixed = format!("{number:.6}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// What a node shows of what came out of it.
fn shown(out: &Option<Value>) -> String {
    match out {
        Some(Value::Number(number)) => format!("= {}", written(*number)),
        Some(Value::Words(words)) => words.clone(),
        None => "Needs A and B".into(),
    }
}

/// The editor's window: the buttons that add nodes, and the board with the graph on it.
pub fn describe(app: &mut Sums, ui: &mut Ui) {
    let room = ui.size();
    ui.row().pad(8.0).gap(6.0).open();
    for (does, name) in KINDS {
        // (A node added by a button goes in the middle of what is in sight.)
        if ui.button(name).clicked {
            let sight = app.graph.board.sight(room);
            add(app, does, pt(sight.x + sight.w / 2.0 - 80.0, sight.y + sight.h / 2.0 - 60.0));
        }
    }
    ui.close();
    let mut changed = false;
    while let Some(n) = ui.graph(&mut app.graph, "Sums") {
        changed |= body(ui, &mut app.graph, n);
        ui.close();
    }
    match app.graph.did {
        Did::Menu { node, at } => {
            app.menu = (node, at);
            let pointer = ui.pointer().unwrap_or_default();
            ui.open_popup(MENU, rect(pointer.x, pointer.y, 0.0, 0.0));
        }
        Did::Nothing | Did::Picked | Did::Moved => {}
        _ => changed = true,
    }
    menu(app, ui);
    if changed {
        work_out(&mut app.graph);
        ui.rerun();
    }
}

/// What the menu of the right button is known by.
const MENU: taut::Id = 0x5e7_0001;

/// The menu of the right button: on the board, the kinds of node to add where it was
/// pressed; on a node, taking it away.
fn menu(app: &mut Sums, ui: &mut Ui) {
    if !ui.menu(MENU) {
        return;
    }
    match app.menu {
        (Some(node), _) => {
            if ui.menu_item("Delete\tDelete").clicked && node < app.graph.nodes.len() {
                app.graph.remove(node);
                work_out(&mut app.graph);
            }
        }
        (None, at) => {
            for (does, name) in KINDS {
                if ui.menu_item(name).clicked {
                    add(app, does, at);
                }
            }
        }
    }
    ui.close();
}

/// Adds a node that does `does` at `at` on the board, picked by itself.
fn add(app: &mut Sums, does: Does, at: Point) {
    app.graph.nodes.iter_mut().for_each(|node| node.picked = false);
    let added = app.graph.add(node(does, at));
    app.graph.nodes[added].picked = true;
    work_out(&mut app.graph);
}

/// What goes in a node under its ports: the number or the words it holds, or what came out
/// of it. Says whether the person changed what it holds.
fn body(ui: &mut Ui, graph: &mut Graph<Step>, n: usize) -> bool {
    let t = ui.theme;
    let caption = match input(graph, n, 1) {
        Some(Value::Words(words)) => words.clone(),
        _ => String::new(),
    };
    let step = &mut graph.nodes[n].data;
    match step.does {
        Does::Number => {
            let number = ui.number(&mut step.number, 1.0);
            ui.tell(&number, Role::Spin, None, "Value");
            number.changed
        }
        Does::Words => ui.text_box(&mut step.words, "Words").changed,
        Does::Show => {
            ui.note(&caption);
            let number = match &step.out {
                Some(Value::Number(number)) => written(*number),
                _ => "Nothing wired in".into(),
            };
            ui.text(&number, Font { size: t.font.size * 1.6, ..t.font }, t.text);
            false
        }
        _ => {
            ui.label(&shown(&step.out));
            false
        }
    }
}

fn main() {
    let mut app = Sums::default();
    let window = Window { title: "Nodes", width: 900.0, height: 600.0, min_width: 480.0, min_height: 320.0, ..Default::default() };
    taut::run(window, move |ui| describe(&mut app, ui));
}

#[cfg(test)]
mod tests {
    use super::*;
    use taut::testing::Trial;

    #[test]
    fn nodes_work_themselves_out_as_they_are_wired_by_the_pointer_and_the_keys() {
        let mut trial = Trial::new(Sums::default(), describe);
        trial.resize(900.0, 600.0, 1.0);
        // 3 and 4 added, under their caption; the node to multiply needs its inputs.
        assert!(trial.has(Role::Text, "7") && trial.has(Role::Text, "Total") && trial.has(Role::Text, "Needs A and B"), "{}", trial.listed());
        // The sum dragged into the first input of the node to multiply (the only input not
        // wired), and the second number into its other one from the keyboard.
        let (from, to) = (trial.spot(Role::Button, "Result, number out, to Show: Value"), trial.spot(Role::Button, "A, number in, not wired"));
        trial.drag(from, to);
        trial.press(Role::Button, "B, number in, not wired");
        trial.key("Enter");
        trial.key("Down");
        trial.key("Down");
        assert!(trial.find(Role::Button, "Value, number out, to Add: B").focused, "{}", trial.listed());
        trial.key("Enter");
        assert!(trial.has(Role::Text, "= 28"), "7 times 4: {}", trial.listed());
        // A number stepped up: everything that comes of it follows.
        trial.press(Role::Spin, "Value");
        trial.key("Up");
        assert!(trial.has(Role::Text, "8") && trial.has(Role::Text, "= 32"), "{}", trial.listed());
    }

    #[test]
    fn the_right_button_adds_a_node_where_it_was_pressed_and_words_do_not_go_into_a_number() {
        let mut trial = Trial::new(Sums::default(), describe);
        trial.resize(900.0, 600.0, 1.0);
        trial.right_click(pt(300.0, 520.0));
        trial.press(Role::MenuItem, "Divide");
        let divide = trial.app.graph.nodes.last().expect("a node").clone();
        assert_eq!((divide.title.as_str(), divide.picked), ("Divide", true));
        let wires = trial.app.graph.wires.len();
        let (from, to) = (trial.spot(Role::Button, "Words, words out, to Show: Caption"), trial.spot(Role::Button, "A, number in, not wired"));
        trial.drag(from, to);
        assert_eq!(trial.app.graph.wires.len(), wires, "words are refused by a number");
        // On a node, the right button takes it away.
        let title = trial.spot(Role::Row, "Divide");
        trial.right_click(title);
        trial.press(Role::MenuItem, "Delete");
        assert!(!trial.app.graph.nodes.iter().any(|node| node.title == "Divide"));
    }
}
