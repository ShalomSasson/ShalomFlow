//! Transforms: run a saved AI rewrite instruction over the user's selection.

use std::sync::atomic::{AtomicBool, Ordering};
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

    let Some(llm) = crate::flow::resolve_flow_llm(&settings) else {
        crate::utils::show_overlay_notice(&app, "transformNoModel");
        return;
    };

    let selection = match crate::selection::capture_selection(&app) {
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

    let generation = tokio::time::timeout(
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
    )
    .await;

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

    // The selection is still active, so a paste replaces it. Like Flow, a
    // transform must never submit or append anything on the user's behalf —
    // it replaces the selection and stops.
    if let Err(err) = crate::clipboard::paste_with_behavior(
        clean,
        app.clone(),
        crate::clipboard::PasteBehavior {
            allow_trailing_space: false,
            allow_auto_submit: false,
        },
    ) {
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
}
