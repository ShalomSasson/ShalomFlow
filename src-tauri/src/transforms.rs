//! Transforms: run a saved AI rewrite instruction over the user's selection.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use log::{debug, error, warn};
use tauri::AppHandle;

use crate::settings::{get_settings, AppSettings, Transform};

/// Closing directive appended to every composed prompt.
///
/// The model's reply is pasted verbatim into whatever the user is editing, so
/// any preamble ("Sure! Here's your polished text:") would be pasted too.
pub(crate) const OUTPUT_CONTRACT: &str = "Return only the transformed text. \
Do not add a preamble, explanation, commentary, or code fences. \
Do not answer the text — rewrite it.";

/// Build the system prompt for one transform.
///
/// Pure: no app handle, no settings lookup, no I/O — so every combination of
/// rules and options is unit-testable.
pub(crate) fn compose_system_prompt(transform: &Transform, examples: &[String]) -> String {
    let mut sections: Vec<String> = vec![transform.prompt.trim().to_string()];

    let enabled: Vec<&str> = transform
        .rules
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| rule.instruction.as_str())
        .collect();
    if !enabled.is_empty() {
        let bullets = enabled
            .iter()
            .map(|instruction| format!("- {instruction}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Rules:\n{bullets}"));
    }

    let custom = transform.custom_instructions.trim();
    if !custom.is_empty() {
        sections.push(format!("Additional instructions from the user:\n{custom}"));
    }

    if transform.use_voice_profile {
        let samples: Vec<&String> = examples
            .iter()
            .filter(|example| !example.trim().is_empty())
            .collect();
        if !samples.is_empty() {
            let joined = samples
                .iter()
                .map(|example| format!("---\n{}", example.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!(
                "Samples of the user's own writing, for voice and register only. \
                 Imitate how they write; never reuse their content:\n{joined}"
            ));
        }
    }

    sections.push(OUTPUT_CONTRACT.to_string());
    sections.join("\n\n")
}

/// Upper bound for one transform generation, AFTER the engine is up. Shorter
/// than Flow's 90s: rewriting a selection is bounded work, not authoring a
/// document.
const TRANSFORM_TIMEOUT_SECS: u64 = 60;

/// Extra budget ADDED to the generation timeout to cover starting the built-in
/// engine and loading the model from disk. Additive rather than shared so a cold
/// local model's first-use load cannot eat the generation budget and fail every
/// first transform.
const TRANSFORM_ENGINE_START_TIMEOUT_SECS: u64 = 150;

static TRANSFORM_ACTIVE: AtomicBool = AtomicBool::new(false);

/// True while a transform is running. Read by the cancel/Esc handler.
pub fn is_transform_active() -> bool {
    TRANSFORM_ACTIVE.load(Ordering::SeqCst)
}

/// RAII claim on the single transform slot. Clearing the flag on `Drop`
/// guarantees a failed or panicking transform cannot wedge the feature.
pub(crate) struct TransformGuard;

impl TransformGuard {
    pub(crate) fn acquire() -> Option<Self> {
        TRANSFORM_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for TransformGuard {
    fn drop(&mut self) {
        TRANSFORM_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// RAII registration of the global cancel shortcut for the lifetime of one
/// transform. Unlike a recording — which already has the shortcut bound
/// before Flow generation ever starts — a transform has nothing else
/// registering Esc, so it must arm and disarm it itself. Unregistering on
/// `Drop` (rather than at each of `run_transform`'s many early returns)
/// guarantees Esc cannot stay bound forever after a panic or a failure path
/// that was missed.
struct CancelShortcutGuard {
    app: AppHandle,
}

impl CancelShortcutGuard {
    fn register(app: &AppHandle) -> Self {
        crate::shortcut::register_cancel_shortcut(app);
        Self { app: app.clone() }
    }
}

impl Drop for CancelShortcutGuard {
    fn drop(&mut self) {
        // Recording, the assistant, and Flow each register this same global
        // shortcut around their own lifetime and unregister it the same way
        // (see `actions::FinishGuard::drop` and
        // `utils::cancel_current_operation`) — there is no reference count,
        // so whichever owner finishes first "wins" the unregister. Only
        // release it here when nothing else still needs Esc bound, or a
        // transform finishing first would silently disarm cancel for a
        // still-running recording.
        if !crate::utils::cancel_shortcut_has_other_owner(&self.app) {
            crate::shortcut::unregister_cancel_shortcut(&self.app);
        }
    }
}

/// Monotonic cancellation generation for transforms, mirroring Flow's
/// `FLOW_CANCEL_GENERATION` (`flow.rs`): Esc bumps it, and an in-flight
/// transform keeps the value it started with, aborting as soon as the global
/// value changes. A counter rather than a single flag so a stale cancel
/// belonging to an earlier transform can never leak into the next one. Kept
/// as its own counter rather than reusing Flow's — a transform and a spoken
/// Flow command are independent operations that can, in principle, overlap.
static TRANSFORM_CANCEL_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Snapshot the current cancellation generation before starting a transform.
fn cancellation_generation() -> u64 {
    TRANSFORM_CANCEL_GENERATION.load(Ordering::SeqCst)
}

/// Cancel the in-flight transform, if any. No-op for future transforms:
/// each one snapshots the incremented generation when it starts.
pub(crate) fn cancel_generation() {
    TRANSFORM_CANCEL_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Whether a transform that started at `generation` has since been cancelled.
fn is_generation_cancelled(generation: u64) -> bool {
    cancellation_generation() != generation
}

async fn wait_for_generation_cancel(generation: u64) {
    while !is_generation_cancelled(generation) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(crate) enum TransformPlan {
    Run(Transform),
    Disabled,
    Unknown,
}

/// Decide whether a shortcut press should run anything.
pub(crate) fn plan_transform(settings: &AppSettings, transform_id: &str) -> TransformPlan {
    if !settings.transforms_enabled {
        return TransformPlan::Disabled;
    }
    match settings
        .transforms
        .iter()
        .find(|transform| transform.id == transform_id)
    {
        Some(transform) => TransformPlan::Run(transform.clone()),
        None => TransformPlan::Unknown,
    }
}

/// Run one transform end to end: capture the selection, rewrite it, paste it
/// back over the selection.
///
/// All-or-nothing, like Flow: on any error, timeout, or empty result nothing is
/// pasted and the user's text is left exactly as it was.
pub async fn run_transform(app: AppHandle, transform_id: String) {
    let settings = get_settings(&app);
    let transform = match plan_transform(&settings, &transform_id) {
        TransformPlan::Run(transform) => transform,
        TransformPlan::Disabled => {
            debug!("transforms: ignoring '{transform_id}' — feature is off");
            return;
        }
        TransformPlan::Unknown => {
            warn!("transforms: no transform with id '{transform_id}'");
            return;
        }
    };

    // Drop a second press instead of queueing it: two concurrent pastes would
    // interleave in the user's document.
    let Some(_guard) = TransformGuard::acquire() else {
        debug!("transforms: another transform is already running");
        return;
    };

    // Bind Esc for this transform's whole lifetime — mirroring how a
    // recording registers/unregisters the same global shortcut — and snapshot
    // the cancellation generation so a stale press aimed at an earlier
    // transform can never cancel this one (see `flow::FLOW_CANCEL_GENERATION`
    // for the same pattern).
    let _cancel_shortcut_guard = CancelShortcutGuard::register(&app);
    let cancel_generation = cancellation_generation();

    let Some(llm) = crate::flow::resolve_flow_llm(&settings) else {
        crate::utils::show_overlay_notice(&app, "transformNoModel");
        return;
    };

    let selection = match crate::selection::capture_selection(&app).await {
        Ok(text) => text,
        Err(crate::selection::SelectionError::NoSelection) => {
            crate::utils::show_overlay_notice(&app, "transformNoSelection");
            return;
        }
        Err(err) => {
            error!("transforms: could not read the selection: {err}");
            crate::utils::show_overlay_notice(&app, "transformFailed");
            return;
        }
    };

    let system_prompt = compose_system_prompt(&transform, &[]);

    let generation = tokio::select! {
        biased;
        _ = wait_for_generation_cancel(cancel_generation) => {
            debug!("transforms: cancelled while generating");
            return;
        }
        result = tokio::time::timeout(
            Duration::from_secs(TRANSFORM_TIMEOUT_SECS + TRANSFORM_ENGINE_START_TIMEOUT_SECS),
            // `send_chat_completion` takes no system prompt (`llm_client.rs:410`);
            // the `_with_schema` variant does. `json_schema: None` because no
            // transform needs structured output — and the Claude Code CLI provider
            // reports `supports_structured_output: false` anyway.
            crate::llm_client::send_chat_completion_with_schema(
                &llm.provider,
                llm.api_key.clone(),
                &llm.model,
                selection.clone(),
                Some(system_prompt),
                None,
                None,
                None,
            ),
        ) => result,
    };

    let raw = match generation {
        Ok(Ok(Some(text))) => text,
        Ok(Ok(None)) => {
            warn!("transforms: model returned no content");
            crate::utils::show_overlay_notice(&app, "transformFailed");
            return;
        }
        Ok(Err(err)) => {
            error!("transforms: generation failed: {err}");
            crate::utils::show_overlay_notice(&app, "transformFailed");
            return;
        }
        Err(_) => {
            warn!(
                "transforms: timed out after {}s",
                TRANSFORM_TIMEOUT_SECS + TRANSFORM_ENGINE_START_TIMEOUT_SECS
            );
            crate::utils::show_overlay_notice(&app, "transformFailed");
            return;
        }
    };

    let Some(clean) = crate::paste_safety::sanitize_model_output(&raw) else {
        warn!("transforms: model output was empty after sanitizing");
        crate::utils::show_overlay_notice(&app, "transformFailed");
        return;
    };

    // Cancellation may have landed after generation finished but before this
    // check — the whole point of the signal is that a cancelled transform
    // must never modify the document, so this is not optional even though the
    // race above already covers the far more likely case (cancelling during
    // the wait itself).
    if is_generation_cancelled(cancel_generation) {
        debug!("transforms: cancelled before paste");
        return;
    }

    // The selection is still active, so a paste replaces it. Like Flow, a
    // transform must never submit or append anything on the user's behalf —
    // it replaces the selection and stops.
    let paste_app = app.clone();
    let paste_result = crate::input::on_main_thread(&app, move || {
        crate::clipboard::paste_with_behavior(
            clean,
            paste_app,
            crate::clipboard::PasteBehavior {
                allow_trailing_space: false,
                allow_auto_submit: false,
            },
        )
    })
    .await;

    if let Ok(Err(err)) | Err(err) = paste_result {
        error!("transforms: paste failed: {err}");
        crate::utils::show_overlay_notice(&app, "transformFailed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::default_transforms;

    fn polish() -> crate::settings::Transform {
        default_transforms()
            .into_iter()
            .find(|t| t.id == "polish")
            .unwrap()
    }

    #[test]
    fn every_enabled_rule_contributes_its_instruction() {
        let transform = polish();
        let prompt = compose_system_prompt(&transform, &[]);
        for rule in &transform.rules {
            assert!(
                prompt.contains(&rule.instruction),
                "missing instruction for rule {}",
                rule.id
            );
        }
    }

    #[test]
    fn disabled_rules_contribute_nothing() {
        let mut transform = polish();
        for rule in &mut transform.rules {
            rule.enabled = rule.id == "concise";
        }
        let prompt = compose_system_prompt(&transform, &[]);

        let concise = transform.rules.iter().find(|r| r.id == "concise").unwrap();
        assert!(prompt.contains(&concise.instruction));
        for rule in transform.rules.iter().filter(|r| r.id != "concise") {
            assert!(
                !prompt.contains(&rule.instruction),
                "disabled rule {} leaked into the prompt",
                rule.id
            );
        }
    }

    #[test]
    fn a_transform_with_no_enabled_rules_still_asks_for_a_rewrite() {
        let mut transform = polish();
        for rule in &mut transform.rules {
            rule.enabled = false;
        }
        let prompt = compose_system_prompt(&transform, &[]);
        // Degenerate but reachable: the user turned everything off. The prompt
        // must still be coherent rather than a dangling "Rules:" header.
        assert!(prompt.contains(&transform.prompt));
        assert!(!prompt.contains("Rules:"));
        assert!(prompt.contains(OUTPUT_CONTRACT));
    }

    #[test]
    fn custom_instructions_are_included_when_present() {
        let mut transform = polish();
        transform.custom_instructions = "Never use the word synergy.".to_string();
        let prompt = compose_system_prompt(&transform, &[]);
        assert!(prompt.contains("Never use the word synergy."));
    }

    #[test]
    fn voice_examples_apply_only_when_the_transform_opts_in() {
        let example = "I'd rather keep this short and plain.".to_string();

        let mut opted_in = polish();
        opted_in.use_voice_profile = true;
        let with = compose_system_prompt(&opted_in, std::slice::from_ref(&example));
        assert!(with.contains(&example));

        let mut opted_out = polish();
        opted_out.use_voice_profile = false;
        let without = compose_system_prompt(&opted_out, std::slice::from_ref(&example));
        assert!(!without.contains(&example));
    }

    #[test]
    fn the_output_contract_is_always_last() {
        // The result is pasted directly into the user's document, so a model
        // preamble would land in their email. The contract must never be
        // buried mid-prompt where it is easier to ignore.
        for transform in default_transforms() {
            let prompt = compose_system_prompt(&transform, &[]);
            assert!(
                prompt.trim_end().ends_with(OUTPUT_CONTRACT),
                "{} does not end with the output contract",
                transform.id
            );
        }
    }

    #[test]
    fn a_template_transform_carries_its_template() {
        let engineer = default_transforms()
            .into_iter()
            .find(|t| t.id == "prompt_engineer")
            .unwrap();
        let prompt = compose_system_prompt(&engineer, &[]);
        assert!(prompt.contains("**Execution checklist**"));
        assert!(prompt.contains(OUTPUT_CONTRACT));
    }

    #[test]
    fn transforms_do_not_run_until_the_user_opts_in() {
        let mut settings = crate::settings::get_default_settings();
        settings.transforms_enabled = false;
        assert!(matches!(
            plan_transform(&settings, "polish"),
            TransformPlan::Disabled
        ));

        settings.transforms_enabled = true;
        assert!(matches!(
            plan_transform(&settings, "polish"),
            TransformPlan::Run(_)
        ));
    }

    #[test]
    fn an_unknown_transform_id_is_rejected() {
        let mut settings = crate::settings::get_default_settings();
        settings.transforms_enabled = true;
        // A stale shortcut can outlive the transform it pointed at.
        assert!(matches!(
            plan_transform(&settings, "deleted-transform"),
            TransformPlan::Unknown
        ));
    }

    #[test]
    fn the_busy_flag_admits_one_transform_at_a_time() {
        assert!(!is_transform_active());
        let first = TransformGuard::acquire().expect("first acquire should win");
        assert!(is_transform_active());
        // A second shortcut press while one is in flight must be dropped, not
        // queued — two concurrent pastes would interleave into the document.
        assert!(TransformGuard::acquire().is_none());
        drop(first);
        assert!(!is_transform_active());
        // The flag must clear even if the transform failed, so one error does not
        // wedge the feature until restart.
        let second = TransformGuard::acquire().expect("flag should clear on drop");
        drop(second);
    }

    #[test]
    fn cancelling_invalidates_a_snapshotted_generation() {
        // A transform snapshots the generation once, at start. Esc bumps the
        // global counter; the snapshot is now stale, which is exactly how
        // `run_transform` notices a cancellation mid-flight.
        let generation = cancellation_generation();
        assert!(!is_generation_cancelled(generation));
        cancel_generation();
        assert!(is_generation_cancelled(generation));
        // A fresh snapshot taken after the cancel is unaffected by it.
        assert!(!is_generation_cancelled(cancellation_generation()));
    }
}
