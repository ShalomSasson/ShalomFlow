/* eslint-disable i18next/no-literal-string -- the "example of transformed
 * text" blocks render the transform's own English sample data (see
 * StylesSettings for the same convention). */
import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Undo2, X } from "lucide-react";
import Badge from "../../ui/Badge";
import { Button } from "../../ui/Button";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { ShortcutInput } from "../ShortcutInput";
import { useSettings } from "../../../hooks/useSettings";
import { useSettingsStore } from "../../../stores/settingsStore";
import { formatKeyCombination, type OSType } from "../../../lib/utils/keyboard";
import { useOsType } from "../../../hooks/useOsType";
import type { Transform } from "@/bindings";

const AUTOSAVE_DELAY_MS = 400;

/** Shortcut chips ("⌥ Opt", "1") from the live binding, transform fallback. */
const KeyChips: React.FC<{ combo: string; osType: OSType }> = ({
  combo,
  osType,
}) => (
  <span className="flex items-center gap-1">
    {formatKeyCombination(combo, osType)
      .split(" + ")
      .filter(Boolean)
      .map((part, i) => (
        <kbd
          key={i}
          className="inline-flex items-center rounded-md border border-hairline-strong bg-surface-strong px-1.5 py-0.5 text-[11px] font-medium text-ink"
        >
          {part}
        </kbd>
      ))}
  </span>
);

interface TransformDetailModalProps {
  open: boolean;
  transformId: string;
  /** Create-your-own layout: name field, no rules, discard-if-empty close. */
  isCreate?: boolean;
  onClose: () => void;
}

/**
 * Two-pane transform editor mirroring the reference: a static left pane
 * (title, shortcut, description, example of transformed text) and an
 * autosaving right pane (shortcut recorder, rule toggles, prompt, shared
 * writing examples). Edits debounce into settings.transforms; the shortcut
 * itself goes through ShortcutInput → change_binding so it re-registers.
 */
export const TransformDetailModal: React.FC<TransformDetailModalProps> = ({
  open,
  transformId,
  isCreate = false,
  onClose,
}) => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const defaultSettings = useSettingsStore((s) => s.defaultSettings);
  const { resetBinding } = useSettings();
  const osType = useOsType();

  const transforms = settings?.transforms ?? [];
  const transform = transforms.find((entry) => entry.id === transformId);

  // Local draft of the text fields so typing doesn't hit disk per keystroke.
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [customInstructions, setCustomInstructions] = useState("");
  const [examples, setExamples] = useState<string[]>([]);
  const loadedId = useRef<string | null>(null);

  useEffect(() => {
    if (open && transform && loadedId.current !== transform.id) {
      loadedId.current = transform.id;
      setName(transform.name);
      setPrompt(transform.prompt);
      setCustomInstructions(transform.custom_instructions);
    }
    if (!open) loadedId.current = null;
  }, [open, transform]);

  // Load once per open; local edits are the source of truth afterwards.
  useEffect(() => {
    if (open) setExamples(settings?.transform_writing_examples ?? []);
  }, [open]); // deliberately not re-synced on later settings changes

  // One autosave path for every transform edit.
  const commitTransform = useCallback(
    (patch: Partial<Transform>) => {
      const current = useSettingsStore.getState().settings?.transforms ?? [];
      void updateSetting(
        "transforms",
        current.map((entry) =>
          entry.id === transformId ? { ...entry, ...patch } : entry,
        ),
      );
    },
    [transformId, updateSetting],
  );

  // Debounced text autosave ("Autosave On").
  const pendingText = useRef<Partial<Transform>>({});
  const textTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const queueText = useCallback(
    (patch: Partial<Transform>) => {
      pendingText.current = { ...pendingText.current, ...patch };
      if (textTimer.current) clearTimeout(textTimer.current);
      textTimer.current = setTimeout(() => {
        commitTransform(pendingText.current);
        pendingText.current = {};
      }, AUTOSAVE_DELAY_MS);
    },
    [commitTransform],
  );
  const flushText = useCallback(() => {
    if (textTimer.current) clearTimeout(textTimer.current);
    if (Object.keys(pendingText.current).length > 0) {
      commitTransform(pendingText.current);
      pendingText.current = {};
    }
  }, [commitTransform]);

  const commitExamples = useCallback(
    (next: string[]) => {
      setExamples(next);
      void updateSetting(
        "transform_writing_examples",
        next.filter((example) => example.trim() !== ""),
      );
    },
    [updateSetting],
  );

  const handleClose = useCallback(() => {
    flushText();
    // A create-draft abandoned with no name and no prompt is discarded.
    if (isCreate && name.trim() === "" && prompt.trim() === "") {
      const current = useSettingsStore.getState().settings?.transforms ?? [];
      void updateSetting(
        "transforms",
        current.filter((entry) => entry.id !== transformId),
      );
    }
    onClose();
  }, [flushText, isCreate, name, prompt, transformId, updateSetting, onClose]);

  // Reset a built-in transform (and its shortcut) back to the shipped state.
  const handleReset = useCallback(() => {
    const original = defaultSettings?.transforms?.find(
      (entry) => entry.id === transformId,
    );
    if (!original) return;
    if (textTimer.current) clearTimeout(textTimer.current);
    pendingText.current = {};
    setName(original.name);
    setPrompt(original.prompt);
    setCustomInstructions(original.custom_instructions);
    commitTransform(original);
    void resetBinding(`transform.${transformId}`);
  }, [defaultSettings, transformId, commitTransform, resetBinding]);

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") handleClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, handleClose]);

  if (!open || !transform) return null;

  const bindingCombo =
    settings?.bindings?.[`transform.${transform.id}`]?.current_binding ??
    transform.shortcut;
  const hasRules = transform.rules.length > 0;
  const canReset =
    !isCreate &&
    (defaultSettings?.transforms?.some((entry) => entry.id === transformId) ??
      false);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/55 p-4 backdrop-blur-[2px]"
      onClick={handleClose}
      role="presentation"
    >
      <div
        className="flex max-h-[88vh] w-full max-w-3xl overflow-hidden rounded-2xl border border-hairline-strong bg-surface shadow-[0_24px_80px_-24px_rgba(0,0,0,0.55)]"
        onClick={(event) => event.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        {/* Left pane — static description of the transform */}
        <div className="flex w-2/5 shrink-0 flex-col gap-4 overflow-y-auto p-7 border-e border-hairline">
          <h2 className="font-display text-2xl text-ink">
            {isCreate ? t("transformsPage.detail.createTitle") : name}
          </h2>
          {isCreate ? (
            <p className="text-sm leading-relaxed text-body">
              {t("transformsPage.detail.createSubtitle")}
            </p>
          ) : (
            <>
              {bindingCombo && (
                <span className="inline-flex w-fit items-center gap-1.5 rounded-lg border border-hairline px-2 py-1 text-xs text-muted">
                  <KeyChips combo={bindingCombo} osType={osType} />
                  {t("transformsPage.detail.toUse")}
                </span>
              )}
              <p className="text-sm leading-relaxed text-body">
                {transform.detail_description}
              </p>
              {transform.sample_text.trim() !== "" && (
                <div className="border-t border-hairline pt-4">
                  <h3 className="text-sm font-semibold text-ink">
                    {t("transformsPage.detail.exampleTitle")}
                  </h3>
                  <p className="mt-2 text-xs leading-relaxed text-muted line-through">
                    {transform.sample_text}
                  </p>
                  {/* Static output preview mirroring the reference design. */}
                  {transform.id === "prompt_engineer" && (
                    <div className="mt-3 border-t border-hairline pt-3">
                      <Badge variant="success" className="font-mono">
                        **Title**
                      </Badge>
                      <p className="mt-1.5 rounded-md bg-success/15 p-2 text-xs leading-relaxed text-ink">
                        Write warm, aspirational, concise product descriptions
                        for a skincare brand
                      </p>
                    </div>
                  )}
                </div>
              )}
              <div className="mt-auto pt-4">
                <Button variant="secondary" size="sm" disabled>
                  {t("transformsPage.detail.seeUpdates")}
                </Button>
              </div>
            </>
          )}
        </div>

        {/* Right pane — autosaving editor */}
        <div className="flex min-w-0 flex-1 flex-col gap-5 overflow-y-auto bg-canvas p-6">
          <div className="flex items-center justify-end gap-3">
            <span className="text-xs text-muted-soft">
              {t("transformsPage.detail.autosave")}
            </span>
            {canReset && (
              <button
                type="button"
                className="flex items-center gap-1 text-xs font-semibold text-ink cursor-pointer hover:text-accent transition-colors"
                onClick={handleReset}
              >
                <Undo2 size={12} />
                {t("transformsPage.detail.reset")}
              </button>
            )}
            <button
              type="button"
              aria-label={t("transformsPage.detail.close")}
              className="text-muted hover:text-ink cursor-pointer transition-colors"
              onClick={handleClose}
            >
              <X size={16} />
            </button>
          </div>

          {isCreate && (
            <div>
              <h3 className="mb-2 text-sm font-semibold text-ink">
                {t("transformsPage.detail.nameLabel")}
              </h3>
              <input
                type="text"
                value={name}
                placeholder={t("transformsPage.detail.namePlaceholder")}
                className="w-full rounded-lg border border-hairline-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-muted-soft focus:outline-none focus:border-accent"
                onChange={(event) => {
                  setName(event.target.value);
                  queueText({ name: event.target.value });
                }}
                onBlur={flushText}
              />
            </div>
          )}

          <div>
            <h3 className="mb-2 text-sm font-semibold text-ink">
              {t("transformsPage.detail.chooseShortcut")}
            </h3>
            {/* Ungrouped ShortcutInput draws its own bordered row. */}
            <ShortcutInput
              shortcutId={`transform.${transform.id}`}
              descriptionMode="tooltip"
            />
          </div>

          {hasRules && (
            <div>
              <h3 className="mb-2 text-sm font-semibold text-ink">
                {t("transformsPage.detail.selectRules", { name })}
              </h3>
              {/* Same joined-rows container as SettingsGroup uses. */}
              <div className="bg-surface rounded-2xl border border-hairline divide-y divide-hairline">
                {transform.rules.map((rule) => (
                  <ToggleSwitch
                    key={rule.id}
                    grouped
                    checked={rule.enabled}
                    label={rule.label}
                    onChange={(enabled) =>
                      commitTransform({
                        rules: transform.rules.map((entry) =>
                          entry.id === rule.id ? { ...entry, enabled } : entry,
                        ),
                      })
                    }
                  />
                ))}
              </div>
            </div>
          )}

          <div>
            <h3 className="mb-2 text-sm font-semibold text-ink">
              {hasRules
                ? t("transformsPage.detail.customizeNamedPrompt", { name })
                : isCreate
                  ? t("transformsPage.detail.customizePrompt")
                  : t("transformsPage.detail.customizeTemplatePrompt")}
            </h3>
            <textarea
              value={hasRules ? customInstructions : prompt}
              rows={hasRules ? 4 : 10}
              placeholder={
                isCreate ? t("transformsPage.detail.promptPlaceholder") : ""
              }
              className="w-full resize-y rounded-lg border border-hairline bg-surface px-3 py-2 text-xs leading-relaxed text-ink placeholder:text-muted-soft focus:outline-none focus:border-accent"
              onChange={(event) => {
                if (hasRules) {
                  setCustomInstructions(event.target.value);
                  queueText({ custom_instructions: event.target.value });
                } else {
                  setPrompt(event.target.value);
                  queueText({ prompt: event.target.value });
                }
              }}
              onBlur={flushText}
            />
          </div>

          {!isCreate && (
            <div>
              <h3 className="text-sm font-semibold text-ink">
                {t("transformsPage.detail.addExamples")}
              </h3>
              <p className="mt-1 mb-2 text-xs leading-relaxed text-muted">
                {t("transformsPage.detail.addExamplesHint")}
              </p>
              <div className="flex flex-col gap-2">
                {examples.map((example, index) => (
                  <div key={index} className="flex items-start gap-2">
                    <textarea
                      value={example}
                      rows={2}
                      className="w-full resize-y rounded-lg border border-hairline bg-surface px-3 py-2 text-xs leading-relaxed text-ink focus:outline-none focus:border-accent"
                      onChange={(event) =>
                        setExamples(
                          examples.map((entry, i) =>
                            i === index ? event.target.value : entry,
                          ),
                        )
                      }
                      onBlur={() => commitExamples(examples)}
                    />
                    <button
                      type="button"
                      aria-label={t("transformsPage.detail.removeExample")}
                      className="mt-2 text-muted-soft hover:text-error cursor-pointer transition-colors"
                      onClick={() =>
                        commitExamples(examples.filter((_, i) => i !== index))
                      }
                    >
                      <X size={14} />
                    </button>
                  </div>
                ))}
              </div>
              <Button
                variant="secondary"
                size="sm"
                className="mt-2"
                onClick={() => setExamples([...examples, ""])}
              >
                <Plus size={12} className="me-1" />
                {t("transformsPage.detail.addExample")}
              </Button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
