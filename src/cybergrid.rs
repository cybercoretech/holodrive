//! CYBERGRID HTTP adapter over Cybercore's shared theme document catalog.
//!
//! Holodrive serves the same built-in and user-created themes as Cyberdeck
//! Hub, Cyberdesk, DaemonHall and Cybercore Theme Studio. It only reads the
//! catalog: the theme picker is per device (remembered in the browser), so
//! choosing a theme on the phone never repaints the desktop.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};

use crate::web::AppState;

pub async fn theme_events(
    State(s): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut stopping = s.stopping.clone();
    let events = stream::unfold(None, |last_revision| async move {
        loop {
            if let Ok(catalog) = catalog() {
                let revision = catalog.revision();
                if last_revision != Some(revision) {
                    let event = Event::default()
                        .event("theme-change")
                        .data(revision.to_string());
                    return Some((Ok(event), Some(revision)));
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    // The stream never ends on its own; end it when the server stops, or
    // graceful shutdown waits on every open tab until systemd kills us.
    let events = events.take_until(async move {
        let _ = stopping.wait_for(|stop| *stop).await;
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

fn catalog() -> Result<cybercore::theme::ThemeCatalog, cybercore::theme::ThemeError> {
    cybercore::theme::ThemeCatalog::load()
}

fn error(status: StatusCode, message: impl ToString) -> Response {
    (status, Json(json!({ "error": message.to_string() }))).into_response()
}

fn palette_json(palette: &cybercore::schema::Palette) -> Value {
    json!({
        "bg": palette.bg,
        "white": palette.white,
        "acid_green": palette.acid_green,
        "hot_pink": palette.hot_pink,
        "purple": palette.purple,
        "cyan": palette.cyan,
        "orange": palette.orange,
        "red": palette.red,
        "panel": palette.panel,
        "line": palette.line,
        "muted": palette.muted,
    })
}

pub async fn list_themes() -> Response {
    let Ok(catalog) = catalog() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not load theme catalog",
        );
    };
    let themes: Vec<Value> = catalog
        .iter()
        .map(|(id, entry)| {
            let document = &entry.document;
            json!({
                "id": id,
                "name": document.metadata.name,
                "family": document.metadata.family,
                "description": document.metadata.description,
                "author": document.metadata.author,
                "builtin": entry.builtin,
                "palette": palette_json(&document.palette),
                "variants": document.variants,
                "design": document.design,
            })
        })
        .collect();
    Json(json!({
        "active": catalog.active_id(),
        "appearance": catalog.active_appearance(),
        "themes": themes
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct CssQuery {
    appearance: Option<cybercore::theme::Appearance>,
}

pub async fn theme_css(Path(id): Path<String>, Query(query): Query<CssQuery>) -> Response {
    let Ok(catalog) = catalog() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not load theme catalog",
        );
    };
    let Some(entry) = catalog.get(&id) else {
        return error(StatusCode::NOT_FOUND, "theme not found");
    };
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        entry
            .document
            .to_css(query.appearance.unwrap_or(catalog.active_appearance())),
    )
        .into_response()
}
