//! The page shell and the compiled-in static assets. Everything after
//! first paint is app.js talking to the JSON API.

pub const PAGE: &str = include_str!("../static/index.html");

const UI_CSS: &str = include_str!("../static/ui.css");
const APP_JS: &str = include_str!("../static/app.js");
const FAVICON: &str = include_str!("../static/favicon.svg");
const LOGO: &str = include_str!("../static/holocube.svg");
/// CodeMirror 6 + Vim, pre-gzipped (`editor/build.sh`); loaded only when editing.
pub const EDITOR_JS_GZ: &[u8] = include_bytes!("../static/editor.js.gz");
const FONT_REGULAR: &[u8] = include_bytes!("../static/fonts/jbm-nerd-regular.woff2");
const FONT_BOLD: &[u8] = include_bytes!("../static/fonts/jbm-nerd-bold.woff2");
const FONT_ITALIC: &[u8] = include_bytes!("../static/fonts/jbm-nerd-italic.woff2");

pub fn asset(file: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match file {
        "ui.css" => (UI_CSS.as_bytes(), "text/css; charset=utf-8"),
        "app.js" => (APP_JS.as_bytes(), "text/javascript; charset=utf-8"),
        "favicon.svg" => (FAVICON.as_bytes(), "image/svg+xml"),
        "holocube.svg" => (LOGO.as_bytes(), "image/svg+xml"),
        _ => return None,
    })
}

pub fn font(file: &str) -> Option<&'static [u8]> {
    Some(match file {
        "jbm-nerd-regular.woff2" => FONT_REGULAR,
        "jbm-nerd-bold.woff2" => FONT_BOLD,
        "jbm-nerd-italic.woff2" => FONT_ITALIC,
        _ => return None,
    })
}
