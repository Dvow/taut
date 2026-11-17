//! A pointer drawn by the library, for when the app is the whole screen (a display fed as
//! pixels, the mouse fed as events) and no system draws one: the app calls
//! [`ui.show_pointer()`](Ui::show_pointer) last, and the pointer lies over everything in the
//! shape the part under it asks for.
use crate::color::Color;
use crate::event::Cursor;
use crate::geom::pt;
use crate::id::Id;
use crate::layout::Flow;
use crate::ui::{Anchor, Icon, Ui};

/// Tells the pointer's layer apart from the others.
const POINTER: Id = 0x9017_7e12_9017_7e12;
/// The side of the square every shape is drawn in.
const SQUARE: f32 = 24.0;

/// A pointer shape in a 24-unit square, and its hot spot: white with a dark outline, to show on
/// any ground.
fn shape(cursor: Cursor) -> (&'static str, [f32; 2]) {
    match cursor {
        Cursor::Arrow => ("M3 2v17.5l4.6-4.4 3 6.9 2.6-1.1-3-6.8H16.5z", [3.0, 2.0]),
        Cursor::Hand => ("M9 3.5a1.5 1.5 0 0 1 3 0v6l5.3 1.1a2 2 0 0 1 1.6 2.2l-.7 5.5a2.5 2.5 0 0 1-2.5 2.2h-5a3 3 0 0 1-2.4-1.2l-3.7-4.9a1.5 1.5 0 0 1 2.3-1.9L9 14.6z", [10.5, 2.0]),
        Cursor::Text => ("M8 3h8v3.2h-2.4v11.6H16V21H8v-3.2h2.4V6.2H8z", [12.0, 12.0]),
        Cursor::ResizeX => ("M2 12l5-5v3.6h10V7l5 5-5 5v-3.6H7V17z", [12.0, 12.0]),
        Cursor::ResizeY => ("M12 2l5 5h-3.6v10H17l-5 5-5-5h3.6V7H7z", [12.0, 12.0]),
        Cursor::ResizeDown => ("M4 4h7L8.5 6.5l9 9L20 13v7h-7l2.5-2.5-9-9L4 11z", [12.0, 12.0]),
        Cursor::ResizeUp => ("M20 4v7l-2.5-2.5-9 9L11 20H4v-7l2.5 2.5 9-9L13 4z", [12.0, 12.0]),
        Cursor::Move => ("M12 2l3.8 3.8h-2.2v4.6h4.6V8.2L22 12l-3.8 3.8v-2.2h-4.6v4.6h2.2L12 22l-3.8-3.8h2.2v-4.6H5.8v2.2L2 12l3.8-3.8v2.2h4.6V5.8H8.2z", [12.0, 12.0]),
        Cursor::No => ("M12 2.5a9.5 9.5 0 1 0 0 19a9.5 9.5 0 0 0 0-19zM6.61 8.73L15.27 17.39A6.3 6.3 0 0 1 6.61 8.73zM8.73 6.61A6.3 6.3 0 0 1 17.39 15.27z", [12.0, 12.0]),
    }
}

/// A pointer drawn by the interface.
impl Ui {
    /// Draws the pointer where it is, in the shape the part under it asks for
    /// ([`set_cursor`](Self::set_cursor)), for an interface that is the whole screen with no
    /// system pointer. Call it last, so it lies over everything; it takes no input. The
    /// interface then runs whenever the pointer moves.
    pub fn show_pointer(&mut self) {
        self.pointer_shown = true;
        let Some(at) = self.pointer() else { return };
        let (path, [x, y]) = shape(self.cursor());
        self.layer(POINTER, Anchor::At(pt(at.x - x, at.y - y)), false, Color::NONE);
        // Above every other layer.
        let layer = self.layers.len() - 1;
        self.layers[layer].height = u64::MAX;
        self.el().flow(Flow::Stack).size(SQUARE, SQUARE).open();
        self.el().icon(Icon { path, view: SQUARE, stroke: 0.0 }, SQUARE, Color::WHITE).show();
        self.el().icon(Icon { path, view: SQUARE, stroke: 1.2 }, SQUARE, Color::rgb(0x101216)).show();
        self.close();
    }
}
