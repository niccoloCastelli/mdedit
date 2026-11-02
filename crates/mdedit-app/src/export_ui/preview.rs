//! Live export preview: the PDF layout rendered on a worker 300 ms after
//! the last change to the dialog or the template editor. Only the newest
//! request's result is kept; while a render runs, newer changes wait for
//! it and then render once.

use std::time::{Duration, Instant};

use iced::widget::{Space, column, container, image, row, scrollable, text};
use iced::{Alignment, Background, Border, Element, Length, Padding};
use mdedit_theme::space::*;
use mdedit_theme::{ColorToken as T, Theme};

use super::{UI, UI_SMALL};
use crate::app::Message;
use crate::style::c;

const DEBOUNCE: Duration = Duration::from_millis(300);
/// Pages rendered; long documents show a note for the rest.
pub const MAX_PAGES: usize = 24;
/// Render resolution: a little above 1 px per pt keeps text crisp at the
/// pane's size without large buffers (A4 ≈ 3.3 MB per page).
pub const PIXEL_PER_PT: f32 = 1.6;

#[derive(Debug, Clone)]
pub struct Rendered {
    pub pages: Vec<(image::Handle, u32, u32)>,
    pub page_count: usize,
}

#[derive(Debug)]
pub struct PreviewState {
    /// Bumped on every change.
    generation: u64,
    changed: Instant,
    running: Option<u64>,
    shown_generation: u64,
    pub shown: Option<Result<Rendered, String>>,
}

impl Default for PreviewState {
    fn default() -> Self {
        // Generation 1 is due at once.
        Self { generation: 1, changed: Instant::now() - DEBOUNCE, running: None, shown_generation: 0, shown: None }
    }
}

impl PreviewState {
    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.changed = Instant::now();
    }

    /// The generation to render now, if one is due.
    pub fn due(&mut self) -> Option<u64> {
        let due =
            self.running.is_none() && self.shown_generation != self.generation && self.changed.elapsed() >= DEBOUNCE;
        due.then(|| {
            self.running = Some(self.generation);
            self.generation
        })
    }

    pub fn finished(&mut self, generation: u64, result: Result<mdedit_export::Preview, String>) {
        if self.running == Some(generation) {
            self.running = None;
        }
        self.shown_generation = generation;
        self.shown = Some(result.map(|p| {
            Rendered {
                page_count: p.page_count,
                pages: p
                    .pages
                    .into_iter()
                    .map(|pg| (image::Handle::from_rgba(pg.width, pg.height, pg.rgba), pg.width, pg.height))
                    .collect(),
            }
        }));
    }

    /// Something left to do: the subscription keeps ticking.
    pub fn pending(&self) -> bool {
        self.running.is_some() || self.shown_generation != self.generation
    }

    pub fn updating(&self) -> bool {
        self.pending()
    }
}

/// The preview pane: pages on `bg-subtle`, scaled to `width`.
pub fn view<'a>(t: &'a Theme, p: &'a PreviewState, width: f32, note: Option<&'a str>) -> Element<'a, Message> {
    let muted = c(t, T::TextMuted);
    let status = match &p.shown {
        Some(Ok(r)) => {
            let n = r.page_count;
            if n == 1 { "1 page".to_string() } else { format!("{n} pages") }
        }
        Some(Err(_)) => "Preview failed".into(),
        None => String::new(),
    };
    let mut head = row![text(status).size(UI_SMALL).color(muted)].spacing(SPACE_2).align_y(Alignment::Center);
    if let Some(note) = note {
        head = head.push(text(note).size(UI_SMALL).color(muted));
    }
    head = head.push(Space::new().width(Length::Fill));
    if p.updating() {
        head = head.push(text("Updating…").size(UI_SMALL).color(muted));
    }

    let page_w = (width - 2.0 * SPACE_5).clamp(160.0, 620.0);
    let body: Element<'a, Message> = match &p.shown {
        None => container(text("Rendering preview…").size(UI).color(muted)).center(Length::Fill).into(),
        Some(Err(e)) => {
            container(text(e.clone()).size(UI).color(c(t, T::Danger))).padding(SPACE_5).width(Length::Fill).into()
        }
        Some(Ok(r)) => {
            let mut pages = column![].spacing(SPACE_4).align_x(Alignment::Center).width(Length::Fill);
            for (handle, w, h) in &r.pages {
                let page_h = page_w * *h as f32 / (*w).max(1) as f32;
                pages = pages.push(container(image(handle.clone()).width(page_w).height(page_h)).style(move |_| {
                    container::Style {
                        background: Some(Background::Color(iced::Color::WHITE)),
                        border: Border { color: c(t, T::Border), width: 1.0, radius: 2.0.into() },
                        shadow: iced::Shadow {
                            color: t.shadow.into(),
                            offset: iced::Vector::new(0.0, 2.0),
                            blur_radius: 8.0,
                        },
                        ..container::Style::default()
                    }
                }));
            }
            if r.page_count > r.pages.len() {
                let more = r.page_count - r.pages.len();
                pages = pages.push(text(format!("{more} more pages not shown")).size(UI_SMALL).color(muted));
            }
            scrollable(container(pages).padding(SPACE_5).width(Length::Fill)).height(Length::Fill).into()
        }
    };
    column![container(head).padding(Padding::from([SPACE_2, SPACE_4])), body].height(Length::Fill).into()
}
