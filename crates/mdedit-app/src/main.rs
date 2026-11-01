//! mdedit application shell (DESIGN_SYSTEM 6, SPEC 3, 5, 9).

mod app;
mod command;
mod keymap;
mod palette;
mod session;
mod settings;
mod style;
mod tab;
mod view;
mod watch;

use app::App;

fn main() -> iced::Result {
    let ui_font = style::font(&mdedit_theme::Fonts::default().ui);
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::iced_theme)
        .subscription(App::subscription)
        .scale_factor(App::scale_factor)
        .default_font(ui_font)
        .exit_on_close_request(false)
        .window_size((1200.0, 800.0))
        .run()
}
