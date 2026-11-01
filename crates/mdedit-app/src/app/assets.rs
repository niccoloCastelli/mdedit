//! Images in documents (SPEC 6.4): Insert → Image, files dropped on the
//! window and pasted images. In project mode the image is copied into the
//! `assets/` folder next to the document; elsewhere it is linked where it
//! is (a pasted image needs a project, since it has no file of its own).

use mdedit_project::assets::{self, AssetSource};

use super::project::Msg;
use super::*;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

impl App {
    /// Insert → Image with no URL: pick files, then insert them.
    pub(super) fn pick_image(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab() else { return Task::none() };
        let id = tab.id;
        let dir =
            tab.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf).or(self.session.last_dir.clone());
        Task::perform(
            async move {
                let mut d = rfd::AsyncFileDialog::new().add_filter("Images", IMAGE_EXTENSIONS);
                if let Some(dir) = dir {
                    d = d.set_directory(dir);
                }
                d.pick_files().await.map(|v| v.iter().map(|h| h.path().to_path_buf()).collect()).unwrap_or_default()
            },
            move |paths| Message::Project(Msg::InsertImages(id, paths)),
        )
    }

    /// Files dropped on the window: images go into the document, markdown
    /// files open in tabs.
    pub(super) fn on_dropped(&mut self, path: PathBuf) -> Task<Message> {
        if is_image(&path) {
            let Some(id) = self.active_tab().filter(|t| t.editable()).map(|t| t.id) else { return Task::none() };
            return self.insert_images(id, vec![path]);
        }
        Task::done(Message::OpenPicked(vec![path]))
    }

    /// Link (or, in a project, import) `paths` and insert them at the cursor.
    pub(super) fn insert_images(&mut self, id: TabId, paths: Vec<PathBuf>) -> Task<Message> {
        let project_assets =
            self.project.as_ref().map(|p| (p.project.root.clone(), p.project.assets_dir().to_string()));
        let Some(tab) = self.tab_mut(id) else { return Task::none() };
        if !tab.editable() || paths.is_empty() {
            return Task::none();
        }
        let md = tab.doc.path.clone();
        let mut links = Vec::new();
        let mut errors = Vec::new();
        for path in &paths {
            let link = match (&project_assets, &md) {
                (Some((root, dir)), Some(md)) if md.starts_with(root) => {
                    assets::import(md, dir, AssetSource::File(path)).map(|l| l.to_string_lossy().replace('\\', "/"))
                }
                _ => assets::link_in_place(md.as_deref(), path).map_err(Into::into),
            };
            match link {
                Ok(l) => links.push(l),
                Err(e) => errors.push(format!("Could not add {}: {e}", path.display())),
            }
        }
        self.insert_links(id, &links);
        for e in errors {
            self.toast_error(e);
        }
        Task::none()
    }

    fn insert_links(&mut self, id: TabId, links: &[String]) {
        let Some(tab) = self.tab_mut(id) else { return };
        for link in links {
            let cmd = core_cmd::DocCommand::Image { url: Some(link.clone()) };
            let result = core_cmd::run(&tab.doc, tab.selection(), &cmd);
            tab.apply_command(result);
        }
    }

    /// Hook: Ctrl+V with an image (and no text) on the clipboard.
    pub(super) fn paste_image_key(&mut self, key: &Key, modifiers: Modifiers) -> Option<Task<Message>> {
        let is_v = matches!(key.as_ref(), Key::Character(c) if c.eq_ignore_ascii_case("v"));
        if !(is_v && modifiers.command() && !modifiers.alt() && !modifiers.shift()) || self.modal_open() {
            return None;
        }
        let tab = self.active_tab().filter(|t| t.editable())?;
        let id = tab.id;
        Some(Task::perform(
            async move { tokio::task::spawn_blocking(clipboard_png).await.unwrap_or(Ok(None)) },
            move |r| Message::Project(Msg::PastedImage(id, r)),
        ))
    }

    pub(super) fn on_pasted_image(&mut self, id: TabId, result: Result<Option<Vec<u8>>, String>) -> Task<Message> {
        let png = match result {
            Ok(Some(png)) => png,
            Ok(None) => return Task::none(),
            Err(e) => {
                self.toast_error(format!("Could not read the pasted image: {e}"));
                return Task::none();
            }
        };
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return Task::none() };
        let md = tab.doc.path.clone();
        let target = self.project.as_ref().and_then(|p| {
            let md = md.as_ref().filter(|m| m.starts_with(&p.project.root))?;
            Some((md.clone(), p.project.assets_dir().to_string()))
        });
        let Some((md, dir)) = target else {
            self.toast("Pasting images needs a saved file in a project (they are stored in its assets folder).");
            return Task::none();
        };
        match assets::import(&md, &dir, AssetSource::Pasted(&png)) {
            Ok(link) => self.insert_links(id, &[link.to_string_lossy().replace('\\', "/")]),
            Err(e) => self.toast_error(format!("Could not save the pasted image: {e}")),
        }
        Task::none()
    }
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The clipboard's image as PNG, when it holds an image and no text (text
/// is pasted by the document widget itself).
fn clipboard_png() -> Result<Option<Vec<u8>>, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    if cb.get_text().is_ok_and(|t| !t.is_empty()) {
        return Ok(None);
    }
    let img = match cb.get_image() {
        Ok(img) => img,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let rgba = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
        .ok_or("the image data has the wrong size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(Some(out.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_extensions() {
        assert!(is_image(Path::new("/a/b.PNG")));
        assert!(is_image(Path::new("x.jpeg")));
        assert!(!is_image(Path::new("notes.md")));
        assert!(!is_image(Path::new("noext")));
    }
}
