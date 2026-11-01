//! Resizable separator (DESIGN_SYSTEM 7, "Separator"): a 1 px `border`
//! line inside a 5 px hit area whose sides take the neighbours' colours, so
//! it reads as a hairline but is easy to grab. Hover and drag turn the line
//! `accent` and 2 px wide.

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget};
use iced::{Border, Color, Element, Event, Length, Rectangle, Size};

pub const HIT: f32 = 5.0;

pub struct Separator<'a, Message> {
    /// Called with the pointer's window x while dragging.
    on_drag: Box<dyn Fn(f32) -> Message + 'a>,
    on_release: Option<Message>,
    left: Color,
    right: Color,
    line: Color,
    active: Color,
}

/// A vertical separator between a `left` and `right` surface.
pub fn vertical<'a, Message>(
    left: Color,
    right: Color,
    line: Color,
    active: Color,
    on_drag: impl Fn(f32) -> Message + 'a,
) -> Separator<'a, Message> {
    Separator { on_drag: Box::new(on_drag), on_release: None, left, right, line, active }
}

impl<Message> Separator<'_, Message> {
    pub fn on_release(mut self, message: Message) -> Self {
        self.on_release = Some(message);
        self
    }
}

#[derive(Default)]
struct State {
    hovered: bool,
    dragging: bool,
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for Separator<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(HIT), Length::Fill)
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fixed(HIT), Length::Fill, Size::ZERO))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let st = tree.state.downcast_mut::<State>();
        let over = cursor.is_over(layout.bounds());
        match event {
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if over != st.hovered {
                    st.hovered = over;
                    shell.request_redraw();
                }
                if st.dragging {
                    shell.publish((self.on_drag)(position.x));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorLeft) if st.hovered => {
                st.hovered = false;
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
                st.dragging = true;
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if st.dragging => {
                st.dragging = false;
                if let Some(m) = &self.on_release {
                    shell.publish(m.clone());
                }
                shell.capture_event();
                shell.request_redraw();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let st = tree.state.downcast_ref::<State>();
        if st.dragging || cursor.is_over(layout.bounds()) {
            mouse::Interaction::ResizingHorizontally
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let st = tree.state.downcast_ref::<State>();
        let b = layout.bounds();
        let mid = b.x + (HIT / 2.0).floor();
        let mut fill = |x: f32, w: f32, color: Color| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(iced::Point::new(x, b.y), Size::new(w, b.height)),
                    border: Border::default(),
                    ..renderer::Quad::default()
                },
                color,
            );
        };
        fill(b.x, mid - b.x, self.left);
        fill(mid, b.x + b.width - mid, self.right);
        if st.hovered || st.dragging {
            fill(mid - 0.5, 2.0, self.active);
        } else {
            fill(mid, 1.0, self.line);
        }
    }
}

impl<'a, Message: Clone + 'a> From<Separator<'a, Message>> for Element<'a, Message> {
    fn from(s: Separator<'a, Message>) -> Self {
        Element::new(s)
    }
}
