//! Export the sample document with every built-in template to PDF and DOCX.
//!
//! ```bash
//! cargo run -p mdedit-export --example export_sample -- [OUT_DIR]
//! ```
//!
//! Writes `sample-<template>.pdf` and `.docx` to `OUT_DIR` (default
//! `target/export-sample`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use mdedit_export::{ExportComment, ExportFormat, ExportOptions, Template, export_to_file};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sample_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/sample");
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/export-sample"));
    std::fs::create_dir_all(&out_dir)?;
    let markdown = std::fs::read_to_string(sample_dir.join("sample.md"))?;
    let offset = markdown.find("four steps").unwrap_or(0);
    let options = ExportOptions {
        include_comments: true,
        comments: vec![ExportComment { offset, author: "Niccolo".into(), body: "Check the step count.".into() }],
        title: Some("sample".into()),
        ..Default::default()
    };
    for template in Template::builtins() {
        for format in [ExportFormat::Pdf, ExportFormat::Docx] {
            let out = out_dir.join(format!("sample-{}.{}", template.name.to_lowercase(), format.extension()));
            let start = Instant::now();
            export_to_file(&markdown, &sample_dir, &template, &options, format, &out)?;
            println!("{} ({} ms)", out.display(), start.elapsed().as_millis());
        }
    }
    Ok(())
}
