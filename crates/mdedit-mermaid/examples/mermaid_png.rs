//! Render a Mermaid file to SVG and PNG: `mermaid_png in.mmd out [--dark]`
//! writes `out.svg` and `out.png` (2×).
fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: mermaid_png in.mmd out [--dark]");
        std::process::exit(2);
    };
    let theme = if args.next().as_deref() == Some("--dark") {
        mdedit_theme::Theme::ember_dark()
    } else {
        mdedit_theme::Theme::ember_light()
    };
    let src = std::fs::read_to_string(input).expect("read input");
    let style = mdedit_mermaid::Style::from_theme(&theme);
    let started = std::time::Instant::now();
    let d = mdedit_mermaid::render(&src, &style).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("rendered in {:?}: {}x{}", started.elapsed(), d.width, d.height);
    std::fs::write(format!("{out}.svg"), &d.svg).expect("write svg");
    std::fs::write(format!("{out}.png"), mdedit_mermaid::to_png(&d, 2.0).expect("png")).expect("write png");
}
