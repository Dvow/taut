//! Node graphs: wired by pointer and keyboard, selected, moved, deleted and copied, told to
//! screen readers, and described only as far as visible. Every step goes through `common`'s
//! window, which compares incremental redraws with full ones.
mod common;
use common::*;
use taut::geom::{Point, pt, rect};
use taut::{Button, Event, Graph, GraphAction, Key, Mods, Node, Role, Ui, Wire};

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
    did: Vec<GraphAction>,
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
    if app.graph.action != GraphAction::None {
        app.did.push(app.graph.action);
    }
}

/// Where a port's dot is in the window: an output (`out`) or an input of a node, while the
/// canvas lies at the top left corner of the window.
fn port(app: &App, node: usize, port: usize, out: bool) -> Point {
    let node = &app.graph.nodes[node];
    let at = pt(node.position.x + if out { node.width } else { 0.0 }, node.position.y + 29.0 + (port as f32 + 0.5) * 22.0);
    app.graph.view.to_window(at, rect(0.0, 0.0, 410.0, 290.0))
}

/// Where a node's title is in the window.
fn title(app: &App, node: usize) -> Point {
    let node = &app.graph.nodes[node];
    app.graph.view.to_window(pt(node.position.x + 40.0, node.position.y + 14.0), rect(0.0, 0.0, 410.0, 290.0))
}

/// Presses the left button at `from` with `mods` held, takes the pointer to `to` and lets go.
fn drag_with(ui: &mut Window, app: &mut App, from: Point, to: Point, mods: Mods) {
    for event in [Event::Move(from), Event::Press(Button::Left, mods), Event::Move(to), Event::Release(Button::Left)] {
        does(ui, event, |ui| show(ui, app));
    }
}

/// The name of what the keyboard is on.
fn focused_name(ui: &Window) -> String {
    focused_node(ui).map_or(String::new(), |found| found.name)
}
/// A pixel of the window, without how solid it is.
fn pixel(ui: &Window, at: Point) -> u32 {
    ui.screen.pixels[at.y as usize * ui.screen.width + at.x as usize] & 0xff_ffff
}

#[test]
fn only_the_nodes_in_sight_are_described_and_each_is_told_with_its_ports() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.wire((0, 0), (2, 0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    assert_eq!(app.shown, ["One", "Two", "Sum"], "the node far off is not described");
    // A node is a group, its title a row of the graph that is selected or not, each port a
    // button that says its kind and what it is wired to.
    assert_eq!(expect_node(&ui, "Sum").role, Role::Group);
    let row = expect_node_as(&ui, Role::Row, "Sum");
    assert_eq!((row.on, row.position), (Some(false), Some([3, 4])));
    assert_eq!(expect_node(&ui, "A, number in, from One: Value").role, Role::Button);
    assert_eq!(expect_node(&ui, "Value, number out, to Sum: A").role, Role::Button);
    expect_node(&ui, "Label, text in, not wired");
    // The parts the app put in a node are in its group.
    let sum = ui.visible_access_nodes().iter().position(|found| found.name == "Sum").unwrap();
    assert!(ui.visible_access_nodes().iter().any(|found| found.name == "Add one" && found.parent == Some(sum)), "{:?}", ui.visible_access_nodes());
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
    assert_eq!((app.graph.wires.as_slice(), app.did.as_slice()), ([wire].as_slice(), [GraphAction::Connected(wire)].as_slice()));
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
    app.graph.can_connect = |from, into| from == into || into == "number";
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
    assert_eq!((app.graph.wires.as_slice(), app.did.last()), ([moved].as_slice(), Some(&GraphAction::Connected(moved))));
    // Let go on the ground, it is gone.
    drag(&mut ui, port(&app, 2, 1, false), pt(215.0, 270.0), |ui| show(ui, &mut app));
    assert_eq!((app.graph.wires.len(), app.did.last()), (0, Some(&GraphAction::Disconnected(moved))));
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
    let picked = |app: &App| app.graph.nodes.iter().map(|node| node.selected).collect::<Vec<_>>();
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [true, false, false, false]);
    // With Control, one more; dragging one of them moves both.
    let two = title(&app, 1);
    drag_with(&mut ui, &mut app, two, two, Mods::CTRL);
    assert_eq!(picked(&app), [true, true, false, false]);
    drag(&mut ui, title(&app, 0), pt(title(&app, 0).x + 30.0, title(&app, 0).y + 20.0), |ui| show(ui, &mut app));
    assert_eq!((app.graph.nodes[0].position, app.graph.nodes[1].position), (pt(50.0, 40.0), pt(50.0, 170.0)));
    assert!(app.did.contains(&GraphAction::Moved));
    // A click on the ground selects nothing; a rectangle over it selects what it touches.
    click(&mut ui, pt(225.0, 270.0), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [false; 4]);
    drag(&mut ui, pt(5.0, 5.0), pt(230.0, 280.0), |ui| show(ui, &mut app));
    assert_eq!(picked(&app), [true, true, false, false]);
    // A click on one of several selected, without moving it, selects it alone.
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
    assert_eq!(app.graph.nodes[0].position, pt(30.0, 30.0));
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes.iter().map(|node| node.title.as_str()).collect::<Vec<_>>(), ["Two", "Sum", "Far"]);
    assert_eq!((app.graph.wires.as_slice(), app.did.last()), ([Wire { from: (0, 0), to: (1, 1) }].as_slice(), Some(&GraphAction::Deleted)));
    // Both left in sight, copied, and pasted where the pointer is: with the wire between them.
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    let sum = title(&app, 1);
    drag_with(&mut ui, &mut app, sum, sum, Mods::SHIFT);
    key(&mut ui, Key::Char('c'), Mods::CTRL, |ui| show(ui, &mut app));
    does(&mut ui, Event::Move(pt(60.0, 250.0)), |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes.len(), 5);
    // (Their top left corner where the pointer is: Two's left edge, and Sum's top.)
    assert_eq!((app.graph.nodes[3].position, app.graph.nodes[4].position, app.graph.nodes[3].title.as_str()), (pt(60.0, 340.0), pt(280.0, 250.0), "Two"));
    assert_eq!(app.graph.wires[1], Wire { from: (3, 0), to: (4, 1) });
    assert!(app.graph.nodes[3].selected && app.graph.nodes[4].selected && !app.graph.nodes[0].selected);
    // Control and D: copies beside them. Escape selects nothing; Control and A everything.
    key(&mut ui, Key::Char('d'), Mods::CTRL, |ui| show(ui, &mut app));
    assert_eq!((app.graph.nodes.len(), app.graph.nodes[6].position), (7, pt(304.0, 274.0)));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| show(ui, &mut app));
    assert!(app.graph.nodes.iter().all(|node| !node.selected));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| show(ui, &mut app));
    assert!(app.graph.nodes.iter().all(|node| node.selected));
}

#[test]
fn the_keyboard_wires_a_port_by_going_through_the_ports_that_take_it() {
    let (mut ui, mut app) = (window(1.0), app());
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    click(&mut ui, port(&app, 0, 0, true), |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "Value, number out, not wired");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    assert!(ui.announced().iter().any(|text| text.starts_with("Wiring")), "{:?}", ui.announced());
    // Down goes through the inputs that take a number, the canvas going along; Up back.
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "A, number in, not wired");
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "A, number in, not wired");
    assert!(app.graph.view.x > 2000.0 && app.shown.contains(&"Far".to_string()), "the board shows the node far off: {:?}", app.graph.view);
    key(&mut ui, Key::Up, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "B, number in, not wired");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    let wire = Wire { from: (0, 0), to: (2, 1) };
    assert_eq!((app.graph.wires.as_slice(), app.did.last()), ([wire].as_slice(), Some(&GraphAction::Connected(wire))));
    assert!(ui.announced().contains(&"Wired One: Value to Sum: B".to_string()), "{:?}", ui.announced());
    ui.run(&mut |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "Value, number out, to Sum: B", "the keyboard is back where the wire started");
    // Escape gives up; Delete takes the wires of a port away.
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires, [wire]);
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert!(app.graph.wires.is_empty() && app.did.last() == Some(&GraphAction::Disconnected(wire)));
}

#[test]
fn tab_goes_from_node_to_node_and_the_board_goes_along() {
    let (mut ui, mut app) = (window(1.0), app());
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    // Each node's title, its ports (once: the first of them), and its button, then the next
    // node's.
    let mut names = Vec::new();
    for _ in 0..10 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
        names.push(focused_name(&ui));
    }
    let expected = ["Value, number out, not wired", "Add one", "Two", "Value, number out, not wired", "Add one", "Sum", "A, number in, not wired", "Add one", "Far"];
    assert_eq!(names[..9], expected);
    assert!(app.graph.view.x > 2000.0, "the board shows the node far off: {:?}", app.graph.view);
    assert_eq!(app.shown.first().map(String::as_str), Some("Sum"), "with the one before it, for Shift and Tab");
    // Among a node's ports, Left and Right go round.
    // (Back by Shift and Tab, from Far's port: its title, Sum's button, Sum's ports.)
    let mut names = Vec::new();
    for (key_, mods) in
        [(Key::Tab, Mods::SHIFT), (Key::Tab, Mods::SHIFT), (Key::Tab, Mods::SHIFT), (Key::Right, Mods::NONE), (Key::Right, Mods::NONE), (Key::Right, Mods::NONE), (Key::Left, Mods::NONE)]
    {
        key(&mut ui, key_, mods, |ui| show(ui, &mut app));
        names.push(focused_name(&ui));
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
    assert_eq!(app.did.last(), Some(&GraphAction::Menu { node: None, at: pt(215.0, 270.0) }));
    let sum = title(&app, 2);
    menu(&mut ui, &mut app, sum);
    assert_eq!(app.did.last(), Some(&GraphAction::Menu { node: Some(2), at: title(&app, 2) }));
    assert!(app.graph.nodes[2].selected);
}

#[test]
fn the_parts_in_a_node_work_at_any_zoom() {
    let (mut ui, mut app) = (window(1.0), app());
    ui.set_screen_reader(true);
    app.graph.view.zoom = 1.5;
    settle(&mut ui, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    let button = expect_node(&ui, "Add one").rect;
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
    assert!(app.graph.view.x > 2000.0 && app.graph.view.y > 1500.0, "{:?}", app.graph.view);
    assert!(app.shown.contains(&"Far".to_string()));
    // With all of it in sight, there is none.
    app.graph.nodes.pop();
    app.graph.view = taut::PanZoom::default();
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
    // that the app is not asked to fill; a rectangle over them all selects every one.
    does(&mut ui, Event::Move(pt(180.0, 180.0)), |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('1'), Mods::SHIFT, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    assert!(app.graph.view.zoom < 0.05 && app.shown.is_empty(), "{:?} {:?}", app.graph.view, app.shown);
    drag(&mut ui, pt(2.0, 2.0), pt(405.0, 285.0), |ui| show(ui, &mut app));
    assert!(app.graph.nodes.iter().all(|node| node.selected));
    // The keyboard goes into one: that one is described whole.
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.shown, ["Node 0", "Node 1"]);
}

/// How long a run takes that describes a graph of a thousand nodes again and draws what
/// changed, as the pointer goes back and forth: at its own size; with all of it in sight
/// (where each is a plain box); with the keyboard in it (its wires are then buttons); and on a
/// grid, with 25 frames and a bend in every tenth wire. `cargo test --release --test
/// node_graphs -- --ignored --nocapture how_long`
#[test]
#[ignore]
fn how_long_a_graph_of_a_thousand_nodes_takes() {
    for (case, what) in ["the nodes in sight of 1,000", "all 1,000 nodes in sight", "the keyboard in it", "a grid, 25 frames, 100 bends"].into_iter().enumerate() {
        let (mut ui, mut app) = (window(1.0), thousand());
        if case == 3 {
            app.graph.grid = 20.0;
            app.graph.frames = (0..25).map(|n| taut::Frame::new("Frame", rect((n % 5) as f32 * 1600.0, (n / 5) as f32 * 1000.0, 1500.0, 900.0))).collect();
            let at = |n: usize| pt((n % 40) as f32 * 200.0 + 180.0, (n / 40) as f32 * 200.0 + 100.0);
            app.graph.reroutes = (0..100).map(|n| taut::Reroute { wire: app.graph.wires[n * 10], at: at(n * 10) }).collect();
        }
        settle(&mut ui, |ui| show(ui, &mut app));
        if case == 1 {
            does(&mut ui, Event::Move(pt(180.0, 180.0)), |ui| show(ui, &mut app));
            key(&mut ui, Key::Char('1'), Mods::SHIFT, |ui| show(ui, &mut app));
            settle(&mut ui, |ui| show(ui, &mut app));
        }
        if case == 2 {
            click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
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
        println!("{what}: {} millionths of a second to describe the graph again and draw what changed", best.as_micros());
    }
}

/// A point a share `t` along the wire from `a` to `b`, as the graph draws it: it leaves `a`
/// rightward and comes into `b` from the left.
fn along_wire(a: Point, b: Point, t: f32) -> Point {
    let reach = ((b.x - a.x).abs() / 2.0).max(40.0);
    let (c, d, s) = (pt(a.x + reach, a.y), pt(b.x - reach, b.y), 1.0 - t);
    let [wa, wc, wd, wb] = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    pt(a.x * wa + c.x * wc + d.x * wd + b.x * wb, a.y * wa + c.y * wc + d.y * wd + b.y * wb)
}

/// Clicks twice at `at`, a moment apart: a double click.
fn double_click(ui: &mut Window, app: &mut App, at: Point) {
    for (step, event) in
        [Event::Move(at), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)].into_iter().enumerate()
    {
        ui.set_time(std::time::Duration::from_millis(10_000 + 50 * step as u64));
        does(ui, event, |ui| show(ui, app));
    }
}

/// Whether a pixel next to `at` (or it) is `color`.
fn near(ui: &Window, at: Point, color: taut::Color) -> bool {
    (-1..=1).any(|dy| (-1..=1).any(|dx| pixel(ui, pt(at.x + dx as f32, at.y + dy as f32)) == color.0 & 0xff_ffff))
}

/// The titles of the nodes, in order.
fn titles(app: &App) -> Vec<&str> {
    app.graph.nodes.iter().map(|node| node.title.as_str()).collect()
}

#[test]
fn an_input_the_app_makes_for_several_takes_every_wire_given_it() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.nodes[2] = Node::new("Sum", 240.0, 60.0, 0.0).multi_input("All", "number").input("B", "number");
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 0, 0, true), port(&app, 2, 0, false), |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 1, 0, true), port(&app, 2, 0, false), |ui| show(ui, &mut app));
    let wires = [Wire { from: (0, 0), to: (2, 0) }, Wire { from: (1, 0), to: (2, 0) }];
    assert_eq!(app.graph.wires, wires, "the second goes in beside the first");
    assert_eq!(app.graph.wires_into((2, 0)).collect::<Vec<_>>(), wires);
    // Again from the same output: nothing changes. Into the one-wire input, the second replaces
    // the first.
    drag(&mut ui, port(&app, 0, 0, true), port(&app, 2, 0, false), |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires.len(), 2);
    expect_node(&ui, "All, number in, several, from One: Value, Two: Value");
    // Pressed, it gives up its last wire.
    drag(&mut ui, port(&app, 2, 0, false), pt(215.0, 270.0), |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires, wires[..1]);
}

#[test]
fn on_a_grid_nodes_bends_and_frames_snap_to_it_and_it_is_drawn() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.grid = 20.0;
    settle(&mut ui, |ui| show(ui, &mut app));
    // A line every 20 units, under everything.
    let t = ui.theme;
    assert_eq!(pixel(&ui, pt(200.0, 230.0)), t.line.0 & 0xff_ffff);
    assert_eq!(pixel(&ui, pt(210.0, 230.0)), t.bg.0 & 0xff_ffff);
    let from = title(&app, 0);
    drag(&mut ui, from, pt(from.x + 33.0, from.y + 7.0), |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes[0].position, pt(60.0, 20.0), "20 and 33 is 53: the nearest line is at 60");
    key(&mut ui, Key::Right, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.nodes[0].position, pt(80.0, 20.0), "an arrow moves it a line");
}

#[test]
fn a_frame_carries_the_nodes_in_it_resizes_by_its_corner_and_the_keys_do_both() {
    let (mut ui, mut app) = (window(1.0), app());
    (app.graph.nodes[0].position, app.graph.nodes[1].position) = (pt(20.0, 80.0), pt(20.0, 170.0));
    app.graph.frames.push(taut::Frame::new("Inputs", rect(10.0, 40.0, 190.0, 230.0)));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    // Under the nodes, its title a button.
    let named = expect_node_as(&ui, Role::Button, "Frame: Inputs");
    drag(&mut ui, middle(named.rect), pt(middle(named.rect).x + 30.0, middle(named.rect).y + 10.0), |ui| show(ui, &mut app));
    assert_eq!(app.graph.frames[0].rect, rect(40.0, 50.0, 190.0, 230.0));
    assert_eq!((app.graph.nodes[0].position, app.graph.nodes[1].position, app.graph.nodes[2].position), (pt(50.0, 90.0), pt(50.0, 180.0), pt(240.0, 60.0)), "the two in it go along");
    // Its corner makes it smaller, down to a least size.
    let corner = pt(40.0 + 190.0 - 6.0, 50.0 + 230.0 - 6.0);
    drag(&mut ui, corner, pt(corner.x - 150.0, corner.y - 60.0), |ui| show(ui, &mut app));
    assert_eq!(app.graph.frames[0].rect, rect(40.0, 50.0, 80.0, 170.0));
    // By keys: arrows move it, with Shift they resize it, Delete takes it away (the nodes stay).
    let named = expect_node_as(&ui, Role::Button, "Frame: Inputs");
    click(&mut ui, middle(named.rect), |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Right, Mods::SHIFT, |ui| show(ui, &mut app));
    assert_eq!(app.graph.frames[0].rect, rect(40.0, 60.0, 90.0, 170.0));
    assert_eq!(app.graph.nodes[0].position, pt(50.0, 90.0), "the node no longer has its middle in it");
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert!(app.graph.frames.is_empty() && app.graph.nodes.len() == 4);
    assert_eq!(app.did.last(), Some(&GraphAction::Framed));
}

#[test]
fn control_g_frames_what_is_selected_and_f_shows_it() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('g'), Mods::CTRL, |ui| show(ui, &mut app));
    let frame = app.graph.frames[0].rect;
    let one = app.graph.nodes[0].position;
    assert_eq!((frame.x, frame.y, frame.w), (one.x - 24.0, one.y - 24.0 - 29.0, 160.0 + 48.0), "around it, with room for a title");
    assert_eq!(app.did.last(), Some(&GraphAction::Framed));
    // F: the selection as near as fits, but no nearer than its own size; with none, all of it.
    key(&mut ui, Key::Char('f'), Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.view.zoom, 1.0);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('f'), Mods::NONE, |ui| show(ui, &mut app));
    assert!(app.graph.view.zoom < 0.2, "{:?}", app.graph.view);
}

#[test]
fn a_double_click_bends_a_wire_there_the_bend_moves_and_goes_again() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.wire((0, 0), (2, 0));
    settle(&mut ui, |ui| show(ui, &mut app));
    let (a, b) = (port(&app, 0, 0, true), port(&app, 2, 0, false));
    let on = along_wire(a, b, 0.25);
    double_click(&mut ui, &mut app, on);
    let wire = Wire { from: (0, 0), to: (2, 0) };
    assert_eq!(app.graph.reroutes.len(), 1);
    assert_eq!((app.graph.reroutes[0].wire, app.did.last()), (wire, Some(&GraphAction::Rerouted(wire))));
    // Dragged far down, the wire goes through it there.
    drag(&mut ui, on, pt(on.x, 240.0), |ui| show(ui, &mut app));
    let bend = app.graph.reroutes[0].at;
    assert_eq!(bend, pt(on.x, 240.0));
    let wire_color = taut::Color::series(0, ui.theme.bg);
    assert!(near(&ui, along_wire(a, bend, 0.5), wire_color) && near(&ui, along_wire(bend, b, 0.5), wire_color), "the wire goes through it");
    assert!(!near(&ui, along_wire(a, b, 0.5), wire_color), "and no longer the short way");
    double_click(&mut ui, &mut app, bend);
    assert!(app.graph.reroutes.is_empty());
}

#[test]
fn wires_are_told_to_readers_and_reached_by_tab_followed_bent_and_removed_by_keys() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.wire((0, 0), (2, 0));
    app.graph.wire((0, 0), (2, 1));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| show(ui, &mut app));
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    // The title, its port, then its wires (once: Right goes round them), then its button.
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "Wire, One: Value to Sum: A");
    key(&mut ui, Key::Right, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "Wire, One: Value to Sum: B");
    assert_eq!(focused_node(&ui).unwrap().shortcut, "Delete");
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "Add one");
    // Enter follows a wire to its input.
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| show(ui, &mut app));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    assert_eq!(focused_name(&ui), "A, number in, from One: Value");
    // B bends one in its middle, and the bend is told and moved by arrows; Delete takes it
    // away, then the wire.
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| show(ui, &mut app));
    let wire = focused_name(&ui);
    assert!(wire.starts_with("Wire, One: Value to Sum"), "{wire}");
    key(&mut ui, Key::Char('b'), Mods::NONE, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    assert_eq!(app.graph.reroutes.len(), 1);
    assert!(focused_name(&ui).starts_with("Bend, One: Value to Sum"), "{}", focused_name(&ui));
    let was = app.graph.reroutes[0].at;
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.reroutes[0].at, pt(was.x, was.y + 10.0));
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    ui.run(&mut |ui| show(ui, &mut app));
    assert!(app.graph.reroutes.is_empty() && focused_name(&ui) == wire, "{}", focused_name(&ui));
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(app.graph.wires.len(), 1);
    assert!(ui.announced().iter().any(|said| said.starts_with("Unwired One: Value to Sum")), "{:?}", ui.announced());
}

#[test]
fn every_edit_undoes_and_redoes_and_the_app_can_keep_its_own() {
    let (mut ui, mut app) = (window(1.0), app());
    app.graph.nodes[1].position = pt(20.0, 220.0);
    settle(&mut ui, |ui| show(ui, &mut app));
    let start = app.graph.clone();
    let positions = |app: &App| app.graph.nodes.iter().map(|node| node.position).collect::<Vec<_>>();
    // A wire, a move, two moves by key (one step), a copy beside it, the original deleted (the
    // keyboard is on it).
    drag(&mut ui, port(&app, 0, 0, true), port(&app, 2, 0, false), |ui| show(ui, &mut app));
    let one = title(&app, 0);
    drag(&mut ui, one, pt(one.x + 10.0, one.y), |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Down, Mods::NONE, |ui| show(ui, &mut app));
    key(&mut ui, Key::Char('d'), Mods::CTRL, |ui| show(ui, &mut app));
    key(&mut ui, Key::Delete, Mods::NONE, |ui| show(ui, &mut app));
    assert_eq!(titles(&app), ["Two", "Sum", "Far", "One"]);
    let end = (positions(&app), app.graph.wires.clone());
    let undo = |ui: &mut Window, app: &mut App| key(ui, Key::Char('z'), Mods::CTRL, |ui| show(ui, app));
    undo(&mut ui, &mut app);
    assert_eq!((titles(&app).len(), app.did.last()), (5, Some(&GraphAction::Undone)));
    undo(&mut ui, &mut app);
    assert_eq!(titles(&app).len(), 4);
    undo(&mut ui, &mut app);
    assert_eq!(app.graph.nodes[0].position, pt(30.0, 20.0), "both keys at once");
    undo(&mut ui, &mut app);
    undo(&mut ui, &mut app);
    assert_eq!((positions(&app), app.graph.wires.len()), (start.nodes.iter().map(|node| node.position).collect::<Vec<_>>(), 0));
    assert!(!app.graph.can_undo());
    for _ in 0..5 {
        key(&mut ui, Key::Char('y'), Mods::CTRL, |ui| show(ui, &mut app));
    }
    assert_eq!((positions(&app), app.graph.wires.clone()), end);
    assert_eq!(app.did.last(), Some(&GraphAction::Redone));
    // An edit of the app's own, kept by it: undone as any other, and it drops what was undone.
    undo(&mut ui, &mut app);
    app.graph.save_undo_step();
    app.graph.add(Node::new("New", 300.0, 200.0, 0.0));
    assert!(!app.graph.can_redo());
    undo(&mut ui, &mut app);
    assert!(!titles(&app).contains(&"New"));
}

#[test]
fn a_node_pressed_under_another_comes_on_top_and_its_wires_go_with_it() {
    let (mut ui, mut app) = (window(1.0), app());
    // Two under Sum's corner, wired into it.
    app.graph.nodes[1].position = pt(200.0, 130.0);
    app.graph.wire((0, 0), (2, 0));
    app.graph.wire((1, 0), (2, 1));
    settle(&mut ui, |ui| show(ui, &mut app));
    // Two's title, where Sum does not cover it.
    click(&mut ui, pt(220.0, 144.0), |ui| show(ui, &mut app));
    assert_eq!(titles(&app), ["One", "Sum", "Far", "Two"]);
    assert_eq!(app.graph.wires, [Wire { from: (0, 0), to: (1, 0) }, Wire { from: (3, 0), to: (1, 1) }]);
    assert!(app.graph.nodes[3].selected);
    // One, under nothing, stays where it is; Sum, under Two now, comes up again.
    click(&mut ui, title(&app, 0), |ui| show(ui, &mut app));
    assert_eq!(titles(&app), ["One", "Sum", "Far", "Two"]);
    click(&mut ui, pt(380.0, 74.0), |ui| show(ui, &mut app));
    assert_eq!(titles(&app), ["One", "Far", "Two", "Sum"]);
}

#[test]
fn dragging_near_the_edge_pans_the_view_and_what_is_dragged_stays_under_the_pointer() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    let from = title(&app, 0);
    let edge = pt(400.0, from.y);
    for event in [Event::Move(from), Event::Press(Button::Left, Mods::NONE), Event::Move(edge)] {
        does(&mut ui, event, |ui| show(ui, &mut app));
    }
    let started = app.graph.view.x;
    for step in 1..=5 {
        ui.set_time(std::time::Duration::from_millis(1000 + 50 * step));
        assert!(ui.wake_in().is_some(), "it asks to run again");
        ui.run(&mut |ui| show(ui, &mut app));
    }
    assert!(app.graph.view.x > started + 50.0, "{:?}", app.graph.view);
    let under = app.graph.view.to_window(pt(app.graph.nodes[0].position.x + 40.0, 0.0), rect(0.0, 0.0, 410.0, 290.0));
    assert!((under.x - edge.x).abs() < 1.0, "{under:?}");
    does(&mut ui, Event::Release(Button::Left), |ui| show(ui, &mut app));
    let stopped = app.graph.view.x;
    ui.set_time(std::time::Duration::from_millis(5000));
    ui.run(&mut |ui| show(ui, &mut app));
    assert_eq!(app.graph.view.x, stopped);
}

#[test]
fn a_new_wire_let_go_on_nothing_asks_the_app_for_a_node_there() {
    let (mut ui, mut app) = (window(1.0), app());
    settle(&mut ui, |ui| show(ui, &mut app));
    drag(&mut ui, port(&app, 0, 0, true), pt(215.0, 240.0), |ui| show(ui, &mut app));
    assert_eq!(app.did.last(), Some(&GraphAction::WireDropped { port: (0, 0), output: true, at: pt(215.0, 240.0) }));
    assert!(app.graph.wires.is_empty());
}
