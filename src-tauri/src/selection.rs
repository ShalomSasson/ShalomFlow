//! Capture the text the user has selected in whatever application is focused.

use std::time::Duration;

use log::{debug, warn};
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::input::{send_copy, EnigoState};

/// How long to wait for the focused app to answer the copy chord. Generous
/// enough for a slow Electron app, short enough that a missed copy still feels
/// instant to the user.
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(400);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Zero-width-wrapped marker written to the clipboard before the copy chord, so
/// "nothing was selected" is distinguishable from "the copy succeeded".
fn sentinel() -> String {
    format!(
        "\u{200B}speakoflow-selection-probe-{}\u{200B}",
        std::process::id()
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Captured(String),
    NoSelection,
}

#[derive(Debug)]
pub(crate) enum SelectionError {
    NoSelection,
    Clipboard(String),
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSelection => write!(f, "No text is selected"),
            Self::Clipboard(detail) => write!(f, "Clipboard unavailable: {detail}"),
        }
    }
}

/// Decide what the clipboard contents mean after a copy attempt.
///
/// Anything byte-identical to the sentinel means the focused app copied
/// nothing. Only an exact match counts: text that merely *contains* the
/// sentinel is a real selection.
pub(crate) fn classify(sentinel: &str, observed: Option<&str>) -> Outcome {
    let Some(text) = observed else {
        return Outcome::NoSelection;
    };
    if text == sentinel || text.trim().is_empty() {
        return Outcome::NoSelection;
    }
    Outcome::Captured(text.trim().to_string())
}

/// What arming the sentinel produced: the clipboard's previous contents (so
/// `capture_selection` can restore them), the sentinel value written (for
/// `classify`), and whether the copy chord itself fired successfully.
struct ArmedProbe {
    original: Option<String>,
    probe: String,
    copy_result: Result<(), String>,
}

/// Remember the clipboard, write the sentinel, and fire the copy chord — the
/// only Enigo-driven part of a capture, and so the only part that needs the
/// main thread (see `input::on_main_thread`). Resolving Enigo *before* the
/// clipboard is touched is deliberate: both steps below are fallible, and if
/// either failed after the sentinel was already written, there would be no
/// `restore` in scope yet to put the user's clipboard back, permanently
/// discarding it. Ordering it this way makes that class of bug unreachable
/// rather than merely unlikely.
fn arm_selection_probe(app: &AppHandle) -> ArmedProbe {
    let clipboard = app.clipboard();
    let original = clipboard.read_text().ok();
    let probe = sentinel();

    let copy_result = (|| -> Result<(), String> {
        let enigo_state = app
            .try_state::<EnigoState>()
            .ok_or_else(|| "input not initialized".to_string())?;
        let mut enigo = enigo_state.0.lock().map_err(|_| "input busy".to_string())?;
        clipboard
            .write_text(probe.as_str())
            .map_err(|e| e.to_string())?;
        send_copy(&mut enigo)
    })();

    ArmedProbe {
        original,
        probe,
        copy_result,
    }
}

/// Read the user's current selection from the focused application.
///
/// Saves and restores the user's clipboard around the probe, so running a
/// transform never costs them what they had copied. Only the copy chord
/// itself needs the main thread; polling the clipboard and restoring it
/// afterward are plain I/O with no Enigo involved, so they run directly on
/// the calling task instead of blocking the main thread for the whole
/// `CAPTURE_TIMEOUT` window.
pub(crate) async fn capture_selection(app: &AppHandle) -> Result<String, SelectionError> {
    let arm_app = app.clone();
    let ArmedProbe {
        original,
        probe,
        copy_result,
    } = crate::input::on_main_thread(app, move || arm_selection_probe(&arm_app))
        .await
        .map_err(SelectionError::Clipboard)?;

    let clipboard = app.clipboard();
    let restore = |clipboard: &tauri_plugin_clipboard_manager::Clipboard<tauri::Wry>| {
        if let Some(text) = original.as_deref() {
            let _ = clipboard.write_text(text);
        }
    };

    if let Err(e) = copy_result {
        restore(&clipboard);
        return Err(SelectionError::Clipboard(e));
    }

    let mut waited = Duration::ZERO;
    let mut outcome = Outcome::NoSelection;
    while waited < CAPTURE_TIMEOUT {
        tokio::time::sleep(POLL_INTERVAL).await;
        waited += POLL_INTERVAL;
        let observed = clipboard.read_text().ok();
        if let Outcome::Captured(text) = classify(&probe, observed.as_deref()) {
            outcome = Outcome::Captured(text);
            break;
        }
    }

    restore(&clipboard);

    match outcome {
        Outcome::Captured(text) => {
            debug!("selection: captured {} chars", text.len());
            Ok(text)
        }
        Outcome::NoSelection => {
            warn!("selection: clipboard never changed — nothing selected");
            Err(SelectionError::NoSelection)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "\u{200B}speakoflow-sel-42\u{200B}";

    #[test]
    fn unchanged_clipboard_means_nothing_was_selected() {
        // The copy chord produced nothing, so our sentinel is still sitting
        // there. Treating this as a successful copy would transform whatever
        // the user copied earlier and paste it over their selection.
        assert_eq!(classify(SENTINEL, Some(SENTINEL)), Outcome::NoSelection);
        // An empty or absent clipboard is equally "no selection", never a
        // successful empty capture.
        assert_eq!(classify(SENTINEL, None), Outcome::NoSelection);
        assert_eq!(classify(SENTINEL, Some("")), Outcome::NoSelection);
        assert_eq!(classify(SENTINEL, Some("   \n\t ")), Outcome::NoSelection);
    }

    #[test]
    fn changed_clipboard_is_the_selection() {
        assert_eq!(
            classify(SENTINEL, Some("hello world")),
            Outcome::Captured("hello world".to_string())
        );
    }

    #[test]
    fn selection_keeps_its_internal_whitespace_but_loses_the_edges() {
        // Users select sloppily — trailing spaces and newlines are normal and
        // must not change the text sent to the model. Interior structure is
        // meaningful content and must survive.
        assert_eq!(
            classify(SENTINEL, Some("  line one\n\nline two  \n")),
            Outcome::Captured("line one\n\nline two".to_string())
        );
    }

    #[test]
    fn text_that_merely_contains_the_sentinel_is_still_a_selection() {
        // Only an exact match means "unchanged". A user could plausibly have
        // the sentinel inside a larger copied document.
        let text = format!("before {SENTINEL} after");
        assert_eq!(
            classify(SENTINEL, Some(&text)),
            Outcome::Captured(text.trim().to_string())
        );
    }
}
