//! mdedit application shell (DESIGN_SYSTEM 6, SPEC 3, 5, 9).

mod app;
mod command;
mod context;
mod keymap;
mod palette;
mod session;
mod settings;
mod style;
mod tab;
mod trace;
mod view;
mod watch;

use app::App;

fn main() -> iced::Result {
    trace::start();
    prefer_vulkan();
    let ui_font = style::font(&mdedit_theme::Fonts::default().ui);
    let (settings, _) = settings::Settings::load(&settings::Paths::detect().settings());
    let app = iced::application(App::new, App::update, App::view)
        .title(App::title)
        .theme(App::iced_theme)
        .subscription(App::subscription)
        .scale_factor(App::scale_factor)
        .default_font(ui_font)
        .exit_on_close_request(false)
        .decorations(!settings.appearance.custom_title_bar())
        .window_size((1200.0, 800.0));
    mdedit_theme::fonts().fold(app, |app, bytes| app.font(bytes)).run()
}

/// wgpu's default is every backend, and on Linux bringing up EGL next to
/// Vulkan costs about 150 ms before the first frame. GL alone starts faster
/// but drops frames while typing (edit to frame p99 25-100 ms against 17 ms),
/// so use Vulkan when a driver for it is installed. `WGPU_BACKEND` still
/// overrides.
fn prefer_vulkan() {
    if !cfg!(target_os = "linux") || std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    let has_icd = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .any(|d| std::fs::read_dir(d).is_ok_and(|mut e| e.next().is_some()));
    if has_icd {
        // SAFETY: first thing in main, before any other thread exists.
        unsafe { std::env::set_var("WGPU_BACKEND", "vulkan") };
    }
}
