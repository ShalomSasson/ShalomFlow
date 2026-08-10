//! Transforms: run a saved AI rewrite instruction over the user's selection.

use crate::settings::Transform;

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
}
