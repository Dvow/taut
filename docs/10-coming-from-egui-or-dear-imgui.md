# 10. Coming from egui or Dear ImGui

What carries over, what differs, and the usual calls side by side (egui 0.37, Dear ImGui 1.91).

## The same

- **Immediate mode:** code describes the interface from app state, again and again. No widget
  tree.
- **A part is one call** that reports what happened: `ui.button("Go").clicked`
  (egui `.clicked()`, Dear ImGui `ImGui::Button("Go")`).
- **The app owns state:** parts take `&mut` (Dear ImGui: a pointer).
- **Ids from position;** changing lists need keys: `.key(..)` (Dear ImGui `PushID`, egui
  `push_id`/`id_salt`).
- **Begin and end:** one `ui.close()` for every kind of box (Dear ImGui `Begin`/`End`,
  `TreeNode`/`TreePop`).

## The differences

| | egui / Dear ImGui | taut |
| --- | --- | --- |
| Runs | Dear ImGui: every frame. egui: on input, redrawing everything on every mouse move. | Only when something changes; redraws only that. No `request_repaint`. |
| Layout | Dear ImGui: a cursor (`SameLine`). egui: placed as added. | Recorded, then laid out; centring is one call (`.align`, `.place`, `ui.spacer()`). |
| Nesting | egui: a closure per container. | Open, fill, close. |
| A click's effect | | Shown in the same run, with no frame of delay. |
| Output | GPU triangle lists. | Finished pixels from the CPU; a window, or one texture. |
| Text | A font atlas. | The system's text engine, or app fonts. |
| Screen readers | egui: AccessKit. Dear ImGui: none. | UI Automation, AT-SPI. |
| Ids | Dear ImGui: the label (`"Save##2"`). | Never the label. |
| Saving layout | `imgui.ini`; egui with serde. | One line of text per value; the app stores it. |
| Included | Separate crates or projects (egui_extras, egui_dock, egui_plot, rfd, image; ImPlot). | Tables, docks, charts, file dialog, image formats, system windows. |

## The same calls, side by side

| To do this | egui | Dear ImGui | taut |
| --- | --- | --- | --- |
| A line of text | `ui.label("Hi")` | `ImGui::Text("Hi")` | `ui.label("Hi")` |
| Quieter text | `ui.weak("..")` | `ImGui::TextDisabled("..")` | `ui.note("..")` |
| Text that wraps | `ui.label(..)` in a vertical layout | `ImGui::TextWrapped("..")` | `ui.paragraph("..")` |
| A link | `ui.link("..")` | `ImGui::TextLink("..")` | `ui.link("..")` |
| A button | `if ui.button("Go").clicked() {}` | `if (ImGui::Button("Go")) {}` | `if ui.button("Go").clicked {}` |
| A tick box | `ui.checkbox(&mut on, "Loud")` | `ImGui::Checkbox("Loud", &on)` | `ui.checkbox(&mut on, "Loud")` |
| Radio buttons | `ui.radio_value(&mut size, Size::S, "Small")` | `ImGui::RadioButton("Small", &size, 0)` | `ui.radio(&mut size, Size::S, "Small")` |
| A slider | `ui.add(egui::Slider::new(&mut v, 0.0..=1.0))` | `ImGui::SliderFloat("v", &v, 0.0f, 1.0f)` | `ui.slider(&mut v, 0.0..=1.0)` |
| A number that is dragged | `ui.add(egui::DragValue::new(&mut v).speed(0.1))` | `ImGui::DragFloat("v", &v, 0.1f)` | `ui.drag_value(&mut v, 0.1)` |
| A number typed, with steps | `egui::DragValue` (typed into on a click) | `ImGui::InputFloat("v", &v, 1.0f)` | `ui.number(&mut v, 1.0)` |
| A text box with a hint | `ui.add(egui::TextEdit::singleline(&mut s).hint_text("Name"))` | `ImGui::InputTextWithHint("##s", "Name", buf, size)` | `ui.text_box(&mut s, "Name")` |
| Several lines | `ui.text_edit_multiline(&mut s)` | `ImGui::InputTextMultiline("##s", buf, size)` | `ui.text_area(&mut s, "", (3, 8))` |
| A password | `egui::TextEdit::singleline(&mut s).password(true)` | `ImGuiInputTextFlags_Password` | `ui.password_box(&mut s, "Password")` |
| How far along | `ui.add(egui::ProgressBar::new(0.5))` | `ImGui::ProgressBar(0.5f)` | `ui.progress(0.5)` |
| A drop-down choice | `egui::ComboBox::from_label("x").selected_text(..).show_ui(ui, \|ui\| ..)` | `ImGui::Combo("x", &i, items, count)` | `ui.select(&mut i, &items)` |
| A row that is picked | `ui.selectable_label(picked, "Inbox")` | `ImGui::Selectable("Inbox", picked)` | `ui.selectable(picked, "Inbox")` |
| A line between groups | `ui.separator()` | `ImGui::Separator()` | `ui.separator()` |
| Side by side | `ui.horizontal(\|ui\| ..)` | `ImGui::SameLine()` between | `ui.row().open(); ..; ui.close();` |
| One under the other | `ui.vertical(\|ui\| ..)` | as they come | `ui.column().open(); ..; ui.close();` |
| A grid | `egui::Grid::new("id").show(ui, \|ui\| { ..; ui.end_row(); })` | `ImGui::BeginTable("id", 2)` .. `EndTable()` | `ui.grid(2).open(); ..; ui.close();` |
| Push the rest to the end | `ui.with_layout(egui::Layout::right_to_left(..), \|ui\| ..)` | `ImGui::SameLine(offset)` | `ui.spacer()` |
| A box that scrolls | `egui::ScrollArea::vertical().show(ui, \|ui\| ..)` | `ImGui::BeginChild("id", size)` .. `EndChild()` | `ui.column().grow().scroll().open(); ..; ui.close();` |
| A long list | `egui::ScrollArea::vertical().show_rows(ui, height, count, \|ui, rows\| ..)` | `ImGuiListClipper` | `for row in ui.list(count, height) { .. } ui.close();` |
| A heading that folds | `ui.collapsing("More", \|ui\| ..)` | `if (ImGui::CollapsingHeader("More")) {}` | `if ui.fold(&mut open, "More") {}` |
| A tree | `egui::CollapsingHeader::new("x").show(ui, \|ui\| ..)` | `if (ImGui::TreeNode("x")) { ..; ImGui::TreePop(); }` | `ui.tree(&mut open, "x"); if open { ..; ui.close(); }` |
| Tabs | `ui.selectable_value(&mut tab, ..)` in a row | `ImGui::BeginTabBar` and `BeginTabItem` | `ui.tabs(&mut tab, &names)` |
| A bar of menus | `egui::MenuBar::new().ui(ui, \|ui\| { ui.menu_button("File", \|ui\| ..); })` | `ImGui::BeginMainMenuBar()`, `BeginMenu("File")` | `ui.menu_bar(); if ui.menu_button("File") { ..; ui.close(); } ui.close();` |
| A row of a menu with its keys | `ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S"))` | `ImGui::MenuItem("Save", "Ctrl+S")` | `ui.menu_item("Save\tCtrl+S")` |
| A menu at the pointer | `response.context_menu(\|ui\| ..)` | `ImGui::BeginPopupContextItem()` | `if ui.context_menu(&response) { ..; ui.close(); }` |
| A tool tip | `response.on_hover_text("..")` | `ImGui::SetItemTooltip("..")` | `ui.tooltip(&response, "..")` |
| A dialog | `egui::Modal::new(id).show(ctx, \|ui\| ..)` | `ImGui::BeginPopupModal("x")` | `ui.dialog(); ..; ui.close();` |
| A window inside the window | `egui::Window::new("x").open(&mut open).show(ctx, \|ui\| ..)` | `ImGui::Begin("x", &open)` .. `End()` | `if ui.panel(&mut panel, "x") { ..; ui.close(); }` |
| Docked panels | the egui_dock crate | the docking branch: `DockSpace` | `ui.dock(&mut dock)` and `ui.docked(&mut dock, name)` |
| A table | `egui_extras::TableBuilder` | `ImGui::BeginTable`, `TableSetupColumn`, `TableHeadersRow` | `ui.table(&mut table, &columns)` and `ui.rows(..)` |
| A chart | the egui_plot crate | ImPlot; `ImGui::PlotLines` | `ui.chart(..)`; `ui.plot_lines(..)` |
| A colour picker | `ui.color_edit_button_srgba(&mut c)` | `ImGui::ColorEdit4("c", col)` | `ui.color_picker(&mut c)` |
| Parts switched off | `ui.add_enabled(false, ..)` | `ImGui::BeginDisabled()` .. `EndDisabled()` | `ui.set_enabled(false)` .. `ui.set_enabled(true)` |
| Telling items of a list apart | `ui.push_id(id, \|ui\| ..)` | `ImGui::PushID(id)` .. `PopID()` | `ui.row().key(id).open()` |
| A key | `ui.input(\|i\| i.key_pressed(egui::Key::Enter))` | `ImGui::IsKeyPressed(ImGuiKey_Enter)` | `ui.key(Key::Enter, Mods::NONE)`, `ui.keys("Ctrl+S")` |
| Put the keyboard on a part | `response.request_focus()` | `ImGui::SetKeyboardFocusHere()` | `ui.set_focus(response.id)` |
| Copy text | `ui.ctx().copy_text(text)` | `ImGui::SetClipboardText(text)` | `ui.copy(text)` |
| A picture | `ui.image(..)` | `ImGui::Image(texture, size)` | `ui.picture(picture)` |
| A drawing of the app's own | `ui.painter().line_segment(..)` | `ImGui::GetWindowDrawList()->AddLine(..)` | `ui.el().size(w, h).outline(&path, 2.0, color).show()` |
| Run again later | `ctx.request_repaint_after(duration)` | (it runs every frame) | `ui.wake_after(duration)` |
| A number that slides | `ctx.animate_value_with_time(id, target, time)` | by hand | `ui.animate(id, target, duration)` |
| Run again, from another thread | `ctx.request_repaint()` on a clone of the context | (it runs every frame) | `taut::Waker.wake()` |
| A button with a picture and words | `egui::Button::image_and_text(img, "Open")` | `ImGui::ImageButton` (no words) | `ui.button_with_picture(picture, "Open")`, `ui.button_with_icon(icon, "Save")` |
| A slider that shows its value | `Slider::new(..).suffix("°")` | `SliderFloat` with a format | `ui.slider_with_value(&mut v, 0.0..=100.0, "°")` |
| How far along, with its percentage | `ProgressBar::new(0.5).show_percentage()` | `ImGui::ProgressBar(0.5f)` | `ui.progress_with_percent(0.5)` |
| A number dragged within a range | `DragValue::new(&mut v).range(0..=10)` | `ImGui::DragInt("v", &v, 0.1f, 0, 10)` | `ui.drag_value_in(&mut v, 0..=10, 0.1)` |
| A drop-down of the app's own values | `ComboBox` with `selectable_value` | by index | `ui.select_value(&mut v, &[(V::A, "A"), (V::B, "B")])` |
| A grid with stripes | `Grid::new(..).striped(true)` | `ImGuiTableFlags_RowBg` | `ui.grid(2).stripes(color).open()` |
| A group drawn see-through | `ui.multiply_opacity(0.5)` | `PushStyleVar(ImGuiStyleVar_Alpha, 0.5f)` | `ui.row().opacity(0.5).open()` |
| Hidden, keeping its room | `ui.add_visible(false, ..)` | by hand | `ui.el().hidden(true).open()` |

## One window in all three

A name and a button that greets. egui:

```text
egui::CentralPanel::default().show(ctx, |ui| {
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut self.name);
        if ui.button("Greet").clicked() {
            self.greeting = format!("Hello, {}!", self.name);
        }
    });
    ui.label(&self.greeting);
});
```

Dear ImGui:

```cpp
ImGui::Begin("Greeter");
ImGui::InputText("##name", name, sizeof name);
ImGui::SameLine();
if (ImGui::Button("Greet"))
    snprintf(greeting, sizeof greeting, "Hello, %s!", name);
ImGui::Text("%s", greeting);
ImGui::End();
```

taut:

```rust
use taut::Ui;

struct App {
    name: String,
    greeting: String,
}

fn describe(app: &mut App, ui: &mut Ui) {
    ui.row().pad(12.0).gap(8.0).open();
    ui.text_box(&mut app.name, "Your name");
    if ui.button("Greet").clicked {
        app.greeting = format!("Hello, {}!", app.name);
    }
    ui.close();
    ui.column().pad((12.0, 0.0)).open();
    ui.label(&app.greeting);
    ui.close();
}
```

(The egui and Dear ImGui snippets are not compiled by the tests.)

## Habits to drop

- **Requesting repaints:** a run that changed something is drawn at its end. Ask only for what
  the user didn't cause (`wake_after`, `Waker`).
- **Ids from labels:** keys tell items apart.
- **Measuring to centre:** say where things go (`align`, `place`, `spacer`).
- **A frame of delay:** a click shows its effect in the same run; `ui.rerun()` for other late
  changes.
- **Setting up a renderer:** `taut::run` opens and draws a window by itself (chapter 9 for
  existing renderers).

## Not here yet

Gamepads; windows on macOS, iOS and Android (egui also runs on the web). See
[what is missing](what-is-missing.md) and
[beside Dear ImGui](beside-other-libraries.md#beside-dear-imgui).

That ends the book. The [guide](../GUIDE.md) has every call on one page; the
[documents index](README.md) has the rest.
