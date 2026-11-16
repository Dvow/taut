//! Node graphs: wired by pointer and keyboard, selected, moved, deleted and copied, told to
//! screen readers, and described only as far as visible. Every step goes through `common`'s
//! window, which compares incremental redraws with full ones.
mod common;
use common::*;
use taut::geom::{Point, pt, rect};
use taut::{Button, Did, Event, Graph, Key, Mods, Node, Role, Said, Ui, Wire};

/// Two numbers and a node that adds them, and one far off; with what kinds there are.
fn sums() -> Graph<f32> {
    let mut graph = Graph::new();
    graph.kinds = vec!["number", "text"];
    graph.add(Node::new("One", 20.0, 20.0, 1.0).output("Value", "number"));
    graph.add(Node::new("Two", 20.0, 150.0, 2.0).output("Value", "number").output("Name", "text"));
    graph.add(Node::new("Sum", 240.0, 60.0, 0.0).input("A", "number").input("B", "number").input("Label", "text"));
    graph.add(Node::new("Far", 3000.0, 2000.0, 0.0).input("A", "number"));
    graph
}

/// What the app keeps: its graph, the titles of the nodes it was given to describe in the
/// last go, and what the person did in each go of the last run.
struct App {
    graph: Graph<f32>,
    shown: Vec<String>,
    did: Vec<Did>,
    pressed: u32,
}

fn app() -> App {
    App { graph: sums(), shown: Vec::new(), did: Vec::new(), pressed: 0 }
}

/// The graph, each node holding its number and a button.
fn show(ui: &mut Ui, app: &mut App) {
    app.shown.clear();
    while let Some(n) = ui.graph(&mut app.graph, "Sums") {
        app.shown.push(app.graph.nodes[n].title.clone());
        ui.label(&app.graph.nodes[n].data.to_string());
        if ui.button("Add one").clicked {
            (app.graph.nodes[n].data, app.pressed) = (app.graph.nodes[n].data + 1.0, app.pressed + 1);
        }
        ui.close();
    }
    if app.graph.did != Did::Nothing {
        app.did.push(app.graph.did);
    }
}

/// Where a port's dot is in the window: an output (`out`) or an input of a node, while the
/// board lies at the top left corner of the window.
fn port(app: &App, node: usize, port: usize, out: bool) -> Point {
    let node = &app.graph.nodes[node];
    let at = pt(node.at.x + if out { node.width } else { 0.0 }, node.at.y + 29.0 + (port as f32 + 0.5) * 22.0);
    app.graph.board.spot(at, rect(0.0, 0.0, 410.0, 290.0))
}

/// Where a node's title is in the window.
fn title(app: &App, node: usize) -> Point {
    let node = &app.graph.nodes[node];
    app.graph.board.spot(pt(node.at.x + 40.0, node.at.y + 14.0), rect(0.0, 0.0, 410.0, 290.0))
}

/// Presses the left button at `from` with `mods` held, takes the pointer to `to` and lets go.
fn drag_with(ui: &mut Window, app: &mut App, from: Point, to: Point, mods: Mods) {
    for event in [Event::Move(from), Event::Press(Button::Left, mods), Event::Move(to), Event::Release(Button::Left)] {
        does(ui, event, |ui| show(ui, app));
    }
}

/// The thing a screen reader is told of with this name.
fn told<'a>(ui: &'a Window, name: &str) -> &'a Said {
    ui.said().iter().find(|said| said.name == name).unwrap_or_else(|| panic!("nothing is told as {name:?}: {:?}", ui.said().iter().map(|said| &said.name).collect::<Vec<_>>()))
}
/// The name of what the keyboard is on.
fn focused(ui: &Window) -> String {
    ui.said().iter().find(|said| said.focused).map_or(String::new(), |said| said.name.clone())
}
/// A pixel of the window, without how solid it is.
fn pixel(ui: &Window, at: Point) -> u32 {
    ui.screen.pixels[at.y as usize * ui.screen.width + at.x as usize] & 0xff_ffff
}

#[test]
fn only_the_nodes_in_sight_are_described_and_each_is_told_with_its_ports() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.wire((0, 0), (2, 0));
    ui.listen(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    assert_eq!(app.shown, ["One", "Two", "Sum"], "the node far off is not described");
    // A node is a group, its title a row of the graph that is picked or not, each port a
    // button that says its kind and what it is wired to.
    assert_eq!(told(&ui, "Sum").role, Role::Group);
    let row = ui.said().iter().find(|said| said.role == Role::Row && said.name == "Sum").expect("the title");
    assert_eq!((row.on, row.of), (Some(false), Some([3, 4])));
    assert_eq!(told(&ui, "A, number in, from One: Value").role, Role::Button);
    assert_eq!(told(&ui, "Value, number out, to Sum: A").role, Role::Button);
    told(&ui, "Label, text in, not wired");
    // The parts the app put in a node are in its group.
    let sum = ui.said().iter().position(|said| said.name == "Sum").unwrap();
    assert!(ui.said().iter().any(|said| said.name == "Add one" && said.within == Some(sum)), "{:?}", ui.said());
}

#[test]
fn a_wire_is_drawn_from_an_output_into_an_input_that_takes_it_and_shows_where_it_goes() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    let (from, to) = (port(&app, 0, 0, true), port(&app, 2, 0, false));
    let t = ui.theme;
    assert_ne!(pixel(&ui, to), t.accent.0 & 0xff_ffff);
    for event in [Event::Move(from), Event::Press(Button::Left, Mods::NONE), Event::Move(to)] {
        does(&mut ui, event, |ui| show(ui, &mut app));
    }
    // While it is drawn over the input that takes it, that input stands out.
    assert_eq!(pixel(&ui, to), t.accent.0 & 0xff_ffff);
    does(&mut ui, Event::Release(Button::Left), |ui| show(ui, &mut app));
    let wire = Wire { from: (0, 0), to: (2, 0) };
    assert_eq!((app.graph.wires.as_slice(), app.did.as_slice()), ([wire].as_slice(), [Did::Wired(wire)].as_slice()));
    // A wired input is a filled dot, in its kind's colour.
    assert_eq!(pixel(&ui, to), taut::Color::series(0, t.bg).0 & 0xff_ffff);
}

#[test]
fn a_wire_is_refused_where_the_kinds_do_not_fit_unless_the_app_says_they_do() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    let (name, a) = (port(&app, 1, 1, true), port(&app, 2, 0, false));
    for event in [Event::Move(name), Event::Press(Button::Left, Mods::NONE), Event::Move(a)] {
        does(&mut ui, event, |ui| show(ui, &mut app));
    }
    assert_eq!(ui.cursor(), taut::Cursor::No, "text does not go into a number");
    does(&mut ui, Event::Release(Button::Left), |ui| show(ui, &mut app));
    assert!(app.graph.wires.is_empty() && app.did.is_empty());
    app.graph.fits = |from, into| from == into || into == "number";
    drag(&mut ui, name, a, |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires, [Wire { from: (1, 1), to: (2, 0) }]);
}

#[test]
fn a_wire_pulled_off_its_input_goes_into_another_or_is_gone() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.wire((0, 0), (2, 0));
    settle(&mut ui, |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 2, 0, false), port(&app, 2, 1, false), |ui| show(ui, &mut app));
    let moved = Wire { from: (0, 0), to: (2, 1) };
    assert_eq!((app.graph.wires.as_slice(), app.did.last()), ([moved].as_slice(), Some(&Did::Wired(moved))));
    // Let go on the ground, it is gone.
    drag(&mut ui, port(&app, 2, 1, false), pt(215.0, 270.0), |ui| show(ui, &mut app));
    assert_eq!((app.graph.wires.len(), app.did.last()), (0, Some(&Did::Unwired(moved))));
    // From an input with nothing in it, a wire is drawn back to an output.
    drag(&mut ui, port(&app, 2, 1, false), port(&app, 1, 0, true), |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires, [Wire { from: (1, 0), to: (2, 1) }]);
}

#[test]
fn a_wire_let_go_on_a_node_goes_into_its_first_free_input_that_takes_it() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 0, 0, true), title(&app, 2), |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 1, 0, true), title(&app, 2), |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 1, 1, true), title(&app, 2), |ui| show(ui, &mut app));
    let wires = [Wire { from: (0, 0), to: (2, 0) }, Wire { from: (1, 0), to: (2, 1) }, Wire { from: (1, 1), to: (2, 2) }];
    assert_eq!(app.graph.wires, wires);
}

#[test]
fn nodes_are_picked_by_a_click_and_by_a_rectangle_and_move_together() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    let picked = |app: &App| app.graph.nodes.iter().map(|node| node.picked).collect::<Vec<_>>();
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [true, false, false, false]);
    // With Control, one more; dragging one of them moves both.
    let two = title(&app, 1);
    drag_with(&mut ui, &mut app, two, two, Mods::CTRL);
    assert_eq!(picked(&app), [true, true, false, false]);
    drag(&mut ui, title(&app, 0), pt(title(&app, 0).x + 30.0, title(&app, 0).y + 20.0), |ui| show(ui, &mut app));
    assert_eq!((app.graph.nodes[0].at, app.graph.nodes[1].at), (pt(50.0, 40.0), pt(50.0, 170.0)));
    assert!(app.did.contains(&Did::Moved));
    // A click on the ground picks nothing; a rectangle over it picks what it touches.
    click(&mut ui, pt(225.0, 270.0), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [false; 4]);
    drag(&mut ui, pt(5.0, 5.0), pt(230.0, 280.0), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [true, true, false, false]);
    // A click on one of several picked, without moving it, picks it alone.
    click(&mut ui, title(&app, 1), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [false, true, false, false]);
}

#[test]
fn keys_move_delete_copy_and_paste_the_nodes_picked() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.wire((0, 0), (2, 0));
    app.graph.wire((1, 0), (2, 1));
    settle(&mut ui, |ui| show(ui, &mut app));
    // The keyboard is on the title of what was clicked: the arrows move it.
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    key(&mut ui, Key::Right, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes[0].at, pt(30.0, 30.0));
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes.iter().map(|node| node.title.as_str()).collect::<Vec<_>>(), ["Two", "Sum", "Far"]);
    assert_eq!((app.graph.wires.as_slice(), app.did.last()), ([Wire { from: (0, 0), to: (1, 1) }].as_slice(), Some(&Did::Deleted)));
    // Both left in sight, copied, and pasted where the pointer is: with the wire between them.
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    let sum = title(&app, 1);
    drag_with(&mut ui, &mut app, sum, sum, Mods::SHIFT);
    key(&mut ui, Key::Char('c'), Mods::CTRL, |ui| show(ui, &mut app));
    does(&mut ui, Event::Move(pt(60.0, 250.0)), |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes.len(), 5);
    // (Their top left corner where the pointer is: Two's left edge, and Sum's top.)
    assert_eq!((app.graph.nodes[3].at, app.graph.nodes[4].at, app.graph.nodes[3].title.as_str()), (pt(60.0, 340.0), pt(280.0, 250.0), "Two"));
    assert_eq!(app.graph.wires[1], Wire { from: (3, 0), to: (4, 1) });
    assert!(app.graph.nodes[3].picked && app.graph.nodes[4].picked && !app.graph.nodes[0].picked);
    // Control and D: copies beside them. Escape picks nothing; Control and A everything.
    key(&mut ui, Key::Char('d'), Mods::CTRL, |ui| show(ui, &mut app));
    assert_eq!((app.graph.nodes.len(), app.graph.nodes[6].at), (7, pt(304.0, 274.0)));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| show(ui, &mut app));
    assert!(app.graph.nodes.iter().all(|node| !node.picked));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| show(ui, &mut app));
    assert!(app.graph.nodes.iter().all(|node| node.picked));
}

#[test]
fn the_keyboard_wires_a_port_by_going_through_the_ports_that_take_it() {
    let (mut ui, mut app) = (window(1.0), app());
    ui.listen(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    click(&mut ui, port(&app, 0, 0, true), |ui| show(ui, &mut app));
    assert_eq!(focused(&ui), "Value, number out, not wired");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    assert!(ui.announced().iter().any(|said| said.starts_with("Wiring")), "{:?}", ui.announced());
    // Down goes through the inputs that take a number, the board going along; Up back.
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused(&ui), "A, number in, not wired");
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused(&ui), "A, number in, not wired");
    assert!(app.graph.board.x > 2000.0 && app.shown.contains(&"Far".to_string()), "the board shows the node far off: {:?}", app.graph.board);
    key(&mut ui, Key::Up, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused(&ui), "B, number in, not wired");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    let wire = Wire { from: (0, 0), to: (2, 1) };
    assert_eq!((app.graph.wires.as_slice(), app.did.last()), ([wire].as_slice(), Some(&Did::Wired(wire))));
    assert!(ui.announced().contains(&"Wired One: Value to Sum: B".to_string()), "{:?}", ui.announced());
    ui.run(&mut |ui| show(ui, &mut app));
    assert_eq!(focused(&ui), "Value, number out, to Sum: B", "the keyboard is back where the wire started");
    // Escape gives up; Delete takes the wires of a port away.
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires, [wire]);
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert!(app.graph.wires.is_empty() && app.did.last() == Some(&Did::Unwired(wire)));
}

#[test]
fn tab_goes_from_node_to_node_and_the_board_goes_along() {
    let (mut ui, mut app) = (window(1.0), app());
    ui.listen(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    // Each node's title, its ports (once: the first of them), and its button, then the next
    // node's.
    let mut names = Vec::new();
    for _ in 0..10 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
        names.push(focused(&ui));
    }
    let expected = ["Value, number out, not wired", "Add one", "Two", "Value, number out, not wired", "Add one", "Sum", "A, number in, not wired", "Add one", "Far"];
    assert_eq!(names[..9], expected);
    assert!(app.graph.board.x > 2000.0, "the board shows the node far off: {:?}", app.graph.board);
    assert_eq!(app.shown.first().map(String::as_str), Some("Sum"), "with the one before it, for Shift and Tab");
    // Among a node's ports, Left and Right go round.
    // (Back by Shift and Tab, from Far's port: its title, Sum's button, Sum's ports.)
    let mut names = Vec::new();
    for (key_, mods) in
        [(Key::Tab, Mods::SHIFT), (Key::Tab, Mods::SHIFT), (Key::Tab, Mods::SHIFT), (Key::Right, Mods::NONE), (Key::Right, Mods::NONE), (Key::Right, Mods::NONE), (Key::Left, Mods::NONE)]
    {
        key(&mut ui, key_, mods, |ui| show(ui, &mut app));
        names.push(focused(&ui));
    }
    assert_eq!(names[2..], ["A, number in, not wired", "B, number in, not wired", "Label, text in, not wired", "A, number in, not wired", "Label, text in, not wired"], "{names:?}");
}

#[test]
fn the_right_button_asks_for_the_apps_menu_where_it_was_pressed() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    let menu = |ui: &mut Window, app: &mut App, at: Point| {
        for event in [Event::Move(at), Event::Press(Button::Right, Mods::NONE), Event::Release(Button::Right)] {
            does(ui, event, |ui| show(ui, app));
        }
    };
    menu(&mut ui, &mut app, pt(215.0, 270.0));
    assert_eq!(app.did.last(), Some(&Did::Menu { node: None, at: pt(215.0, 270.0) }));
    let sum = title(&app, 2);
    menu(&mut ui, &mut app, sum);
    assert_eq!(app.did.last(), Some(&Did::Menu { node: Some(2), at: title(&app, 2) }));
    assert!(app.graph.nodes[2].picked);
}

#[test]
fn the_parts_in_a_node_work_at_any_zoom() {
    let (mut ui, mut app) = (window(1.0), app());
    ui.listen(true);
    app.graph.board.zoom = 1.5;
    settle(&mut ui, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    let button = ui.said().iter().find(|said| said.name == "Add one").expect("a button").rect;
    assert!(button.h > 30.0, "as large as the board shows it: {button:?}");
    click(&mut ui, middle(button), |ui| show(ui, &mut app));
    assert_eq!((app.pressed, app.graph.nodes[0].data), (1, 2.0));
}

#[test]
fn a_small_map_of_all_of_it_moves_the_board_where_it_is_pressed() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    // In the bottom right corner, above the bars: the node far off is in its far corner.
    let t = ui.theme;
    let far_corner = pt(410.0 - 8.0 - 8.0, 290.0 - 8.0 - 8.0);
    assert_eq!(pixel(&ui, pt(far_corner.x - 30.0, far_corner.y - 10.0)), t.panel.0 & 0xff_ffff, "the map's ground");
    click(&mut ui, far_corner, |ui| show(ui, &mut app));
    assert!(app.graph.board.x > 2000.0 && app.graph.board.y > 1500.0, "{:?}", app.graph.board);
    assert!(app.shown.contains(&"Far".to_string()));
    // With all of it in sight, there is none.
    app.graph.nodes.pop();
    app.graph.board = taut::Board::default();
    settle(&mut ui, |ui| show(ui, &mut app));
    assert_eq!(pixel(&ui, pt(far_corner.x - 30.0, far_corner.y - 10.0)), t.bg.0 & 0xff_ffff);
}

/// A thousand nodes in rows of forty, each wired into the next, and two hundred units apart.
fn thousand() -> App {
    let mut graph = Graph::new();
    for n in 0..1000 {
        let (x, y) = ((n % 40) as f32 * 200.0, (n / 40) as f32 * 200.0);
        graph.add(Node::new(format!("Node {n}"), x, y, n as f32).input("In", "number").output("Out", "number"));
    }
    for n in 1..1000 {
        graph.wire((n - 1, 0), (n, 0));
    }
    App { graph, shown: Vec::new(), did: Vec::new(), pressed: 0 }
}

#[test]
fn of_a_thousand_nodes_only_those_in_sight_are_described() {
    let (mut ui, mut app) = (window(1.0), thousand());
    settle(&mut ui, |ui| show(ui, &mut app));
    assert_eq!(app.shown, ["Node 0", "Node 1", "Node 2", "Node 40", "Node 41", "Node 42"]);
    // All of it at once: each node is too small to be worth its parts, and is a plain box
    // that the app is not asked to fill; a rectangle over them all picks every one.
    does(&mut ui, Event::Move(pt(180.0, 180.0)), |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('1'), Mods::SHIFT, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    assert!(app.graph.board.zoom < 0.05 && app.shown.is_empty(), "{:?} {:?}", app.graph.board, app.shown);
    drag(&mut ui, pt(2.0, 2.0), pt(405.0, 285.0), |ui| show(ui, &mut app));
    assert!(app.graph.nodes.iter().all(|node| node.picked));
    // The keyboard goes into one: that one is described whole.
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.shown, ["Node 0", "Node 1"]);
}

/// How long a run takes that describes a graph of a thousand nodes again and draws what
/// changed, as the pointer goes back and forth: at its own size, and with all of it in
/// sight (where each is a plain box). `cargo test --release --test graphs -- --ignored --nocapture how_long`
#[test]
#[ignore]
fn how_long_a_graph_of_a_thousand_nodes_takes() {
    for all in [false, true] {
        let (mut ui, mut app) = (window(1.0), thousand());
        settle(&mut ui, |ui| show(ui, &mut app));
        if all {
            does(&mut ui, Event::Move(pt(180.0, 180.0)), |ui| show(ui, &mut app));
            key(&mut ui, Key::Char('1'), Mods::SHIFT, |ui| show(ui, &mut app));
            settle(&mut ui, |ui| show(ui, &mut app));
        }
        let mut best = std::time::Duration::MAX;
        for round in 0..400 {
            ui.ui.event(Event::Move(pt(100.0 + (round % 2) as f32 * 200.0, 40.0)));
            let started = std::time::Instant::now();
            let changed = ui.ui.run(&mut |ui| show(ui, &mut app));
            ui.screen.redraw(&mut ui.ui, changed);
            best = best.min(started.elapsed());
        }
        let what = if all { "all 1,000 nodes in sight" } else { "the nodes in sight of 1,000" };
        println!("{what}: {} millionths of a second to describe the graph again and draw what changed", best.as_micros());
    }
}
