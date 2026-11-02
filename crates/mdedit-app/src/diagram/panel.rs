//! Layout of a diagram editor window: toolbar, canvas, properties, live
//! preview and source (DESIGN_SYSTEM 6, 7).

use iced::widget::{
    Space, button, canvas as canvas_widget, column, container, row, scrollable, stack, svg, text, text_editor,
    text_input, tooltip,
};
use iced::{Alignment, Element, Font, Length, Padding};
use mdedit_mermaid::flowchart::{Direction, EdgeStyle, Head, Line, Shape};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};
use mdedit_view::diagrams::DiagramState;

use super::canvas::{Board, Colors, Glyph, Sel};
use super::{DiagramMsg as Msg, EDGE_LABEL_INPUT, Editor, LABEL_INPUT};
use crate::style::{self, Btn, c};

const UI: f32 = 13.0;
const UI_SMALL: f32 = 12.0;
const EYEBROW: f32 = 11.0;
const SIDE_WIDTH: f32 = 360.0;
const TOOLBAR_HEIGHT: f32 = 44.0;

fn bold() -> Font {
    Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT }
}

fn icon<'a>(t: &'a Theme, name: &str, color: T) -> Element<'a, Msg> {
    match mdedit_theme::icon(name) {
        Some(bytes) => svg(svg::Handle::from_memory(bytes))
            .width(16)
            .height(16)
            .style(move |_, _| svg::Style { color: Some(c(t, color)) })
            .into(),
        None => Space::new().width(16).into(),
    }
}

fn tip<'a>(t: &'a Theme, content: impl Into<Element<'a, Msg>>, tip: &'a str) -> Element<'a, Msg> {
    tooltip(
        content,
        container(text(tip).size(UI_SMALL))
            .padding(Padding::from([SPACE_1, SPACE_2]))
            .style(style::popover(t, RADIUS_SM)),
        tooltip::Position::Bottom,
    )
    .delay(std::time::Duration::from_millis(500))
    .into()
}

/// Toolbar button: icon and label.
fn tool<'a>(t: &'a Theme, name: &str, label: &'a str, hint: &'a str, msg: Option<Msg>) -> Element<'a, Msg> {
    let color = if msg.is_some() { T::TextSecondary } else { T::TextMuted };
    let b = button(row![icon(t, name, color), text(label).size(UI)].spacing(SPACE_2).align_y(Alignment::Center))
        .height(30)
        .padding(Padding::from([SPACE_1, SPACE_3]))
        .style(style::btn(t, Btn::Ghost))
        .on_press_maybe(msg);
    tip(t, b, hint)
}

fn small_button<'a>(t: &'a Theme, label: &'a str, kind: Btn, msg: Msg) -> Element<'a, Msg> {
    button(text(label).size(UI))
        .height(30)
        .padding(Padding::from([SPACE_1, SPACE_4]))
        .style(style::btn(t, kind))
        .on_press(msg)
        .into()
}

fn eyebrow<'a>(t: &'a Theme, label: &'a str) -> Element<'a, Msg> {
    text(label.to_uppercase()).size(EYEBROW).font(bold()).color(c(t, T::TextMuted)).into()
}

fn hline(t: &Theme) -> Element<'_, Msg> {
    container(Space::new()).width(Length::Fill).height(1).style(style::divider(t)).into()
}

fn vline(t: &Theme) -> Element<'_, Msg> {
    container(Space::new()).width(1).height(Length::Fill).style(style::divider(t)).into()
}

/// A row of toggle buttons (segmented control).
fn segments<'a, V: Copy + PartialEq + 'a>(
    t: &'a Theme,
    items: &[(V, &'a str)],
    current: Option<V>,
    on: impl Fn(V) -> Msg + 'a,
) -> Element<'a, Msg> {
    let mut r = row![].spacing(SPACE_1);
    for (v, label) in items {
        let on_it = current == Some(*v);
        r = r.push(
            button(container(text(*label).size(UI_SMALL)).center_x(Length::Fill))
                .width(Length::Fill)
                .height(28)
                .padding(Padding::from([SPACE_1, SPACE_1]))
                .style(style::btn(t, if on_it { Btn::Toggled } else { Btn::Secondary }))
                .on_press(on(*v)),
        );
    }
    r.width(Length::Fill).into()
}

pub fn view<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &ed.model {
        Some(_) => row![canvas_area(ed, t), vline(t), side(ed, t)].height(Length::Fill).into(),
        None => source_only(ed, t),
    };
    let mut col = column![toolbar(ed, t), hline(t), body];
    if ed.confirm_close {
        col = col.push(confirm_bar(t));
    }
    container(col).width(Length::Fill).height(Length::Fill).style(style::app_bg(t)).into()
}

fn toolbar<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    let flow = ed.model.is_some();
    let has_sel = ed.selection != Sel::None;
    let mut r = row![].spacing(SPACE_1).align_y(Alignment::Center);
    if flow {
        r = r
            .push(tool(t, "plus", "Add node", "Add a node (Ctrl+N), connected to the selected one", Some(Msg::AddNode)))
            .push(tool(t, "trash", "Delete", "Delete the selection (Delete)", has_sel.then_some(Msg::Delete)))
            .push(Space::new().width(SPACE_2));
    }
    r = r.push(tool(t, "undo-2", "Undo", "Undo (Ctrl+Z)", ed.can_undo().then_some(Msg::Undo))).push(tool(
        t,
        "redo-2",
        "Redo",
        "Redo (Ctrl+Shift+Z)",
        ed.can_redo().then_some(Msg::Redo),
    ));
    if flow {
        r = r
            .push(Space::new().width(SPACE_2))
            .push(tool(
                t,
                "refresh-cw",
                "Auto layout",
                "Place nodes as the rendered diagram does",
                Some(Msg::AutoLayout),
            ))
            .push(tool(t, "maximize-2", "Fit", "Fit the diagram in the view (Ctrl+0)", Some(Msg::Fit)));
    }
    r = r.push(Space::new().width(Length::Fill));
    if let Some(err) = &ed.error {
        r = r.push(text(err.as_str()).size(UI_SMALL).color(c(t, T::Danger)));
        r = r.push(Space::new().width(SPACE_3));
    } else if ed.dirty() {
        r = r.push(text("Unsaved changes").size(UI_SMALL).color(c(t, T::TextMuted)));
        r = r.push(Space::new().width(SPACE_3));
    }
    r = r.push(small_button(t, "Cancel", Btn::Secondary, Msg::CloseRequested)).push(tip(
        t,
        small_button(t, "Save", Btn::Primary, Msg::Save { close: true }),
        "Save to the document and close (Ctrl+Enter); Ctrl+S saves and keeps editing",
    ));
    container(r)
        .height(TOOLBAR_HEIGHT)
        .width(Length::Fill)
        .padding(Padding::from([0.0, SPACE_3]))
        .center_y(TOOLBAR_HEIGHT)
        .style(style::chrome(t))
        .into()
}

fn canvas_area<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    let Some(model) = &ed.model else { return Space::new().into() };
    let board = Board {
        model,
        positions: &ed.positions,
        sizes: &ed.sizes,
        selection: &ed.selection,
        pan: ed.pan,
        zoom: ed.zoom,
        colors: Colors::new(t),
        font: style::font(&t.fonts.ui),
    };
    let canvas: Element<'a, Msg> =
        Element::from(canvas_widget(board).width(Length::Fill).height(Length::Fill)).map(Msg::Canvas);
    let hint = text(
        "Double-click empty space to add a node · drag the dot (or Shift+drag) to connect · \
         double-click to rename · scroll to pan, Ctrl+scroll to zoom",
    )
    .size(UI_SMALL)
    .color(c(t, T::TextMuted));
    let hint = container(hint).padding(Padding::from([SPACE_1, SPACE_3]));
    let zoom = text(format!("{:.0}%", ed.zoom * 100.0)).size(UI_SMALL).color(c(t, T::TextMuted));
    let footer = container(row![hint, Space::new().width(Length::Fill), container(zoom).padding(SPACE_2)])
        .width(Length::Fill)
        .align_bottom(Length::Fill);
    stack![canvas, footer].width(Length::Fill).height(Length::Fill).into()
}

fn side<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    let model = ed.model.as_ref().expect("side panel is for flowcharts");
    let mut props = column![].spacing(SPACE_3);
    props = props.push(eyebrow(t, "Direction"));
    props = props.push(segments(
        t,
        &[
            (Direction::TopDown, "↓ Down"),
            (Direction::LeftRight, "→ Right"),
            (Direction::BottomUp, "↑ Up"),
            (Direction::RightLeft, "← Left"),
        ],
        Some(model.direction),
        Msg::Direction,
    ));
    match &ed.selection {
        Sel::Node(id) => {
            let node = model.node(id);
            props = props.push(Space::new().height(SPACE_1));
            props = props.push(
                row![
                    eyebrow(t, "Node"),
                    Space::new().width(Length::Fill),
                    text(id.as_str()).size(UI_SMALL).font(style::font(&t.fonts.mono)).color(c(t, T::TextMuted))
                ]
                .align_y(Alignment::Center),
            );
            props = props.push(
                text_input("Label", &ed.label)
                    .id(LABEL_INPUT)
                    .on_input(Msg::Label)
                    .on_submit(Msg::LabelDone)
                    .size(UI)
                    .padding(Padding::from([6.0, SPACE_2]))
                    .style(style::input(t)),
            );
            props = props.push(shape_grid(t, node.map(|n| n.shape)));
            props = props.push(
                button(
                    row![icon(t, "trash", T::Danger), text("Delete node").size(UI).color(c(t, T::Danger))]
                        .spacing(SPACE_2)
                        .align_y(Alignment::Center),
                )
                .padding(Padding::from([SPACE_1, SPACE_2]))
                .style(style::btn(t, Btn::Ghost))
                .on_press(Msg::Delete),
            );
        }
        Sel::Edge(i) => {
            let edge = &model.edges[*i];
            props = props.push(Space::new().height(SPACE_1));
            props = props.push(
                row![
                    eyebrow(t, "Connection"),
                    Space::new().width(Length::Fill),
                    text(format!("{} → {}", edge.from, edge.to))
                        .size(UI_SMALL)
                        .font(style::font(&t.fonts.mono))
                        .color(c(t, T::TextMuted))
                ]
                .align_y(Alignment::Center),
            );
            props = props.push(
                text_input("Label (optional)", &ed.edge_label)
                    .id(EDGE_LABEL_INPUT)
                    .on_input(Msg::EdgeLabel)
                    .on_submit(Msg::LabelDone)
                    .size(UI)
                    .padding(Padding::from([6.0, SPACE_2]))
                    .style(style::input(t)),
            );
            let s = edge.style;
            props = props.push(text("Line").size(UI_SMALL).color(c(t, T::TextSecondary)));
            props = props.push(segments(
                t,
                &[
                    (Line::Solid, "Solid"),
                    (Line::Dotted, "Dotted"),
                    (Line::Thick, "Thick"),
                    (Line::Invisible, "Hidden"),
                ],
                Some(s.line),
                move |line| Msg::EdgeStyle(EdgeStyle { line, ..s }),
            ));
            props = props.push(text("End").size(UI_SMALL).color(c(t, T::TextSecondary)));
            props = props.push(segments(
                t,
                &[(Head::Arrow, "Arrow"), (Head::None, "None"), (Head::Circle, "Circle"), (Head::Cross, "Cross")],
                Some(s.head),
                move |head| Msg::EdgeStyle(EdgeStyle { head, both: s.both && head != Head::None, ..s }),
            ));
            props = props.push(segments(t, &[(false, "One way"), (true, "Both ends")], Some(s.both), move |both| {
                let head = if both && s.head == Head::None { Head::Arrow } else { s.head };
                Msg::EdgeStyle(EdgeStyle { both, head, ..s })
            }));
            props = props.push(
                button(
                    row![icon(t, "trash", T::Danger), text("Delete connection").size(UI).color(c(t, T::Danger))]
                        .spacing(SPACE_2)
                        .align_y(Alignment::Center),
                )
                .padding(Padding::from([SPACE_1, SPACE_2]))
                .style(style::btn(t, Btn::Ghost))
                .on_press(Msg::Delete),
            );
        }
        Sel::None => {
            props =
                props.push(text("Select a node or a connection to edit it.").size(UI_SMALL).color(c(t, T::TextMuted)));
        }
    }
    let props = scrollable(container(props).padding(SPACE_4)).height(Length::FillPortion(5));
    column![
        props,
        hline(t),
        container(column![eyebrow(t, "Preview"), preview(ed, t)].spacing(SPACE_2))
            .padding(SPACE_4)
            .height(Length::FillPortion(4)),
        hline(t),
        container(column![eyebrow(t, "Source"), source_editor(ed, t)].spacing(SPACE_2))
            .padding(SPACE_4)
            .height(Length::FillPortion(4)),
    ]
    .width(SIDE_WIDTH)
    .height(Length::Fill)
    .into()
}

fn shape_grid<'a>(t: &'a Theme, current: Option<Shape>) -> Element<'a, Msg> {
    let mut rows = column![].spacing(SPACE_1);
    for chunk in Shape::ALL.chunks(7) {
        let mut r = row![].spacing(SPACE_1);
        for shape in chunk {
            let on = current == Some(*shape);
            let glyph = Glyph {
                shape: *shape,
                fill: c(t, if on { T::AccentSoft } else { T::Surface }),
                stroke: c(t, if on { T::Accent } else { T::TextSecondary }),
            };
            let b = button(canvas_widget(glyph).width(30).height(18))
                .width(Length::Fill)
                .height(30)
                .padding(Padding::from([6.0, 4.0]))
                .style(style::btn(t, if on { Btn::Toggled } else { Btn::Secondary }))
                .on_press(Msg::Shape(*shape));
            r = r.push(tip(t, b, shape.name()));
        }
        rows = rows.push(r);
    }
    rows.into()
}

fn preview<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut slot = DefaultHasher::new();
    ("diagram-editor", ed.window).hash(&mut slot);
    let state = mdedit_view::diagrams::cache().get(&ed.source, &ed.style, slot.finish());
    let (shown, error) = match state {
        DiagramState::Ready(d) => (Some(d), None),
        DiagramState::Loading { stale } => (stale, None),
        DiagramState::Failed { error, stale } => (stale, Some(error)),
    };
    let image: Element<'a, Msg> = match shown {
        Some(d) => svg(d.handle).width(Length::Fill).height(Length::Fill).into(),
        None => container(text("Rendering…").size(UI_SMALL).color(c(t, T::TextMuted))).center(Length::Fill).into(),
    };
    let mut col =
        column![container(image).padding(SPACE_2).width(Length::Fill).height(Length::Fill).style(move |_| {
            container::Style {
                background: Some(c(t, T::Bg).into()),
                border: iced::Border { color: c(t, T::Border), width: 1.0, radius: RADIUS_MD.into() },
                ..Default::default()
            }
        })]
        .spacing(SPACE_1);
    if let Some(e) = error {
        col = col.push(text(format!("Diagram error: {e}")).size(UI_SMALL).color(c(t, T::Danger)));
    }
    col.into()
}

fn source_editor<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    text_editor(&ed.code)
        .on_action(Msg::Code)
        .font(style::font(&t.fonts.mono))
        .size(UI)
        .height(Length::Fill)
        .padding(SPACE_2)
        .style(move |_, status| {
            let focused = matches!(status, text_editor::Status::Focused { .. });
            text_editor::Style {
                background: c(t, T::Surface).into(),
                border: iced::Border {
                    color: if focused { c(t, T::Accent) } else { c(t, T::BorderStrong) },
                    width: if focused { 2.0 } else { 1.0 },
                    radius: RADIUS_MD.into(),
                },
                placeholder: c(t, T::TextMuted),
                value: c(t, T::Text),
                selection: c(t, T::Selection),
            }
        })
        .into()
}

/// Diagram types without a canvas: source on the left, preview on the right.
fn source_only<'a>(ed: &'a Editor, t: &'a Theme) -> Element<'a, Msg> {
    let note =
        text("The visual editor handles flowcharts. Edit this diagram's source; the preview follows as you type.")
            .size(UI_SMALL)
            .color(c(t, T::TextMuted));
    row![
        container(column![eyebrow(t, "Source"), source_editor(ed, t), note].spacing(SPACE_2))
            .padding(SPACE_4)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
        vline(t),
        container(column![eyebrow(t, "Preview"), preview(ed, t)].spacing(SPACE_2))
            .padding(SPACE_4)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
    ]
    .height(Length::Fill)
    .into()
}

fn confirm_bar(t: &Theme) -> Element<'_, Msg> {
    let r = row![
        icon(t, "triangle-alert", T::WarningText),
        text("Save your changes to the diagram before closing?").size(UI).color(c(t, T::Text)),
        Space::new().width(Length::Fill),
        small_button(t, "Keep editing", Btn::Ghost, Msg::KeepEditing),
        small_button(t, "Discard", Btn::Secondary, Msg::Discard),
        small_button(t, "Save", Btn::Primary, Msg::Save { close: true }),
    ]
    .spacing(SPACE_2)
    .align_y(Alignment::Center);
    container(r)
        .padding(Padding::from([SPACE_2, SPACE_4]))
        .width(Length::Fill)
        .style(style::banner(t, style::Banner::Warning))
        .into()
}
