# Packaging

Platform files for installers and bundles. Icons come from the two SVGs in
`assets/icons/` (`mdedit.svg` app, `mdedit-file-md.svg` markdown file type);
`scripts/gen-icons.sh` regenerates every raster below and in `assets/icons/`.

| Platform | Wiring |
|---|---|
| All | `mdedit-app` sets the window icon from `assets/icons/png/mdedit-256.png` (`main.rs`). |
| Linux | `linux/mdedit.desktop` (`Icon=mdedit`, `MimeType=text/markdown;text/x-markdown;`), `linux/icons/hicolor/` (`apps/mdedit` and `mimetypes/mdedit-markdown` at 16-512 px plus scalable SVG), `linux/mdedit-mime.xml` (defines `text/markdown` where shared-mime-info lacks it and points its icon at `mdedit-markdown`; a theme-neutral name because icon themes like Yaru ship their own `text-markdown`, which shadows hicolor). Install: desktop file to `share/applications/`, the hicolor tree to `share/icons/hicolor/`, the XML to `share/mime/packages/`, then `update-mime-database`, `gtk-update-icon-cache` and `update-desktop-database`. |
| Windows | `crates/mdedit-app/build.rs` embeds `assets/icons/mdedit.ico` in `mdedit.exe` (winresource, Windows targets only). `assets/icons/mdedit-file-md.ico` is for the installer's `.md` association (`DefaultIcon`). |
| macOS | `macos/Info.plist` stub for `mdedit.app`: `CFBundleIconFile` = `mdedit.icns`, `CFBundleDocumentTypes` for md/markdown with `mdedit-file-md.icns`; copy both from `assets/icons/` to `Contents/Resources/`. |

`make deb` (cargo-deb, configured in `crates/mdedit-app/Cargo.toml`) packages the Linux files above with the release binary; dpkg triggers refresh the MIME, icon and desktop caches. The other platforms are not packaged yet.
