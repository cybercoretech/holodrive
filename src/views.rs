//! The page shell and the compiled-in static assets. Everything after
//! first paint is app.js talking to the JSON API.

pub const PAGE: &str = include_str!("../static/index.html");
pub const LINK_PAGE: &str = include_str!("../static/link.html");

const UI_CSS: &str = include_str!("../static/ui.css");
const APP_JS: &str = include_str!("../static/app.js");
const KEYSMITH_JS: &str = include_str!("../static/keysmith.js");
const LINK_JS: &str = include_str!("../static/link.js");
const FAVICON: &str = include_str!("../static/favicon.svg");
const LOGO: &str = include_str!("../static/holocube.svg");
const KEYSMITH_WORDLIST: &[u8] = include_bytes!("../static/vendor/keysmith/eff-large.txt");
/// CodeMirror 6 + Vim, pre-gzipped (`editor/build.sh`); loaded only when editing.
pub const EDITOR_JS_GZ: &[u8] = include_bytes!("../static/editor.js.gz");
const FONT_REGULAR: &[u8] = include_bytes!("../static/fonts/jbm-nerd-regular.woff2");
const FONT_BOLD: &[u8] = include_bytes!("../static/fonts/jbm-nerd-bold.woff2");
const FONT_ITALIC: &[u8] = include_bytes!("../static/fonts/jbm-nerd-italic.woff2");

pub fn asset(file: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match file {
        "ui.css" => (UI_CSS.as_bytes(), "text/css; charset=utf-8"),
        "app.js" => (APP_JS.as_bytes(), "text/javascript; charset=utf-8"),
        "keysmith.js" => (KEYSMITH_JS.as_bytes(), "text/javascript; charset=utf-8"),
        "link.js" => (LINK_JS.as_bytes(), "text/javascript; charset=utf-8"),
        "favicon.svg" => (FAVICON.as_bytes(), "image/svg+xml"),
        "holocube.svg" => (LOGO.as_bytes(), "image/svg+xml"),
        "keysmith-eff-large.txt" => (KEYSMITH_WORDLIST, "text/plain; charset=utf-8"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn keysmith_asset_matches_the_pinned_eff_large_list() {
        let digest = Sha256::digest(KEYSMITH_WORDLIST);
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            hex,
            "6d557f0693958fb5e650b68b5bee585eb82cf4da32965505c789e924743bc522"
        );
        let words = std::str::from_utf8(KEYSMITH_WORDLIST)
            .unwrap()
            .lines()
            .collect::<Vec<_>>();
        assert_eq!(words.len(), 7776);
        assert_eq!(
            words.iter().collect::<std::collections::HashSet<_>>().len(),
            7776
        );
    }
}
