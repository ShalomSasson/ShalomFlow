import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Undo2 } from "lucide-react";
import Badge from "../../ui/Badge";
import { Button } from "../../ui/Button";
import { SectionHeader } from "../../ui/SectionHeader";
import { useSettings } from "../../../hooks/useSettings";
import { formatKeyCombination, type OSType } from "../../../lib/utils/keyboard";
import { useOsType } from "../../../hooks/useOsType";
import { TransformDetailModal } from "./TransformDetailModal";
import type { Transform } from "@/bindings";

/** Shortcut string ("option+1") rendered as key chips ("⌥ Opt", "1"). */
const ShortcutChips: React.FC<{ shortcut: string; osType: OSType }> = ({
  shortcut,
  osType,
}) => {
  if (!shortcut) return null;
  const parts = formatKeyCombination(shortcut, osType).split(" + ");
  return (
    <span className="flex items-center gap-1">
      {parts.map((part, i) => (
        <kbd
          key={i}
          className="inline-flex items-center rounded-md border border-hairline-strong bg-surface-strong px-1.5 py-0.5 text-[11px] font-medium text-ink"
        >
          {part}
        </kbd>
      ))}
    </span>
  );
};

/**
 * Transforms — saved AI rewrite instructions bound to global shortcuts.
 * Mirrors the reference Transforms page: opt-in header, promo banner, and a
 * card per transform from settings.transforms. The opt-in toggle persists
 * transforms_enabled; Reset restores the built-in set. Creating and editing
 * transforms is not built yet — the dashed card is a disabled affordance.
 */
export const TransformsSettings: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting, resetSetting, isUpdating } = useSettings();
  const osType = useOsType();
  const [openId, setOpenId] = useState<string | null>(null);
  const [createId, setCreateId] = useState<string | null>(null);

  const enabled = settings?.transforms_enabled ?? false;
  const transforms = settings?.transforms ?? [];

  // "Autosave On": the create modal edits a persisted draft, so the draft is
  // appended before the modal opens (and discarded on close if left empty).
  const startCreate = () => {
    const draft: Transform = {
      id: `custom-${Date.now()}`,
      name: "",
      description: "",
      detail_description: "",
      prompt: "",
      rules: [],
      custom_instructions: "",
      use_voice_profile: true,
      shortcut: "",
      sample_text: "",
      builtin: false,
    };
    void updateSetting("transforms", [...transforms, draft]);
    setCreateId(draft.id);
  };

  return (
    <div className="max-w-4xl w-full mx-auto space-y-6">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-center gap-2.5">
          <SectionHeader title={t("sidebar.transforms")} />
          <Badge variant="active">{t("transformsPage.beta")}</Badge>
        </div>
        {/* Master opt-in — shortcuts only fire while this is on. Same
            checkbox/peer switch markup as ui/ToggleSwitch, without the row. */}
        <label
          className={`flex items-center gap-2 select-none pt-1 ${
            isUpdating("transforms_enabled")
              ? "cursor-not-allowed"
              : "cursor-pointer"
          }`}
        >
          <span className="text-[13px] text-muted">
            {t("transformsPage.optIn")}
          </span>
          <input
            type="checkbox"
            className="sr-only peer"
            checked={enabled}
            disabled={isUpdating("transforms_enabled")}
            onChange={(e) =>
              void updateSetting("transforms_enabled", e.target.checked)
            }
          />
          <div className="relative w-[42px] h-[26px] bg-hairline-strong peer-focus-visible:outline-none peer-focus-visible:ring-2 peer-focus-visible:ring-accent/40 rounded-full peer peer-checked:after:translate-x-4 rtl:peer-checked:after:-translate-x-4 after:content-[''] after:absolute after:top-0.5 after:start-0.5 after:bg-white after:rounded-full after:h-[22px] after:w-[22px] after:shadow-[0_1px_2px_rgba(0,0,0,0.2)] after:transition-transform after:duration-200 after:ease-out transition-colors duration-200 peer-checked:bg-toggle-track-on peer-checked:after:bg-toggle-knob-on peer-disabled:opacity-50"></div>
        </label>
      </div>

      {/* Promo banner — bg-ink/text-canvas invert with the theme tokens. */}
      <div className="flex items-center justify-between gap-6 rounded-xl bg-ink text-canvas px-6 py-5">
        <div className="min-w-0">
          <h2 className="font-display text-lg leading-snug">
            {t("transformsPage.promo.title")}
          </h2>
          <p className="mt-1 text-xs opacity-75 max-w-md">
            {t("transformsPage.promo.body")}
          </p>
        </div>
        <div className="flex shrink-0 -space-x-1">
          {[
            { label: "M", className: "bg-[#E14B2C]" },
            { label: "C", className: "bg-[#1F1F1F]" },
            { label: "G", className: "bg-[#4F46E5]" },
            { label: "N", className: "bg-[#F2C94C]" },
            { label: "W", className: "bg-[#22C55E]" },
            { label: "S", className: "bg-[#4A154B]" },
          ].map((icon, i) => (
            <span
              key={icon.label}
              className={`flex items-center justify-center w-8 h-8 rounded-lg text-[11px] font-bold text-white ring-2 ring-ink ${icon.className} ${
                i % 2 === 1 ? "translate-y-1" : ""
              }`}
            >
              {icon.label}
            </span>
          ))}
        </div>
      </div>

      <div className="flex items-center justify-between gap-3">
        <h2 className="font-display text-base text-ink">
          {t("transformsPage.myTransforms")}
        </h2>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="flex items-center gap-1.5 text-xs font-semibold text-ink cursor-pointer hover:text-accent transition-colors"
            onClick={() => void resetSetting("transforms")}
          >
            <Undo2 size={12} />
            {t("transformsPage.resetDefaults")}
          </button>
          <Button variant="primary" size="sm" onClick={startCreate}>
            {t("transformsPage.createNew")}
          </Button>
        </div>
      </div>

      <div
        className={`grid grid-cols-3 gap-4 transition-opacity ${
          enabled ? "" : "opacity-45"
        }`}
      >
        {transforms.map((transform) => (
          <button
            key={transform.id}
            type="button"
            className="flex flex-col items-stretch gap-2 rounded-xl border border-hairline bg-surface p-4 min-h-[130px] text-start cursor-pointer hover:border-hairline-strong transition-colors"
            onClick={() => setOpenId(transform.id)}
          >
            <ShortcutChips
              shortcut={
                settings?.bindings?.[`transform.${transform.id}`]
                  ?.current_binding ?? transform.shortcut
              }
              osType={osType}
            />
            <div className="mt-auto">
              <h3 className="text-sm font-semibold text-ink">
                {transform.name}
              </h3>
              <p className="mt-0.5 text-xs text-muted">
                {transform.description}
              </p>
            </div>
          </button>
        ))}

        <button
          type="button"
          className="flex flex-col items-stretch gap-2 rounded-xl border border-dashed border-hairline-strong p-4 min-h-[130px] text-start text-muted-soft cursor-pointer hover:border-accent transition-colors"
          onClick={startCreate}
        >
          <Plus size={14} />
          <div className="mt-auto">
            <h3 className="text-sm font-semibold text-muted">
              {t("transformsPage.createOwn")}
            </h3>
            <p className="mt-0.5 text-xs text-muted-soft">
              {t("transformsPage.uploadPrompt")}
            </p>
          </div>
        </button>
      </div>

      <TransformDetailModal
        open={openId !== null}
        transformId={openId ?? ""}
        onClose={() => setOpenId(null)}
      />
      <TransformDetailModal
        open={createId !== null}
        transformId={createId ?? ""}
        isCreate
        onClose={() => setCreateId(null)}
      />
    </div>
  );
};
