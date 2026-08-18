import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import type { SettingIcon, SettingTone } from "../ui/tones";
import { useSettings } from "../../hooks/useSettings";

interface PolishDictationToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  icon?: SettingIcon;
  tone?: SettingTone;
}

/**
 * Runs the Polish transform over every dictation before pasting. Independent
 * of the Transforms opt-in (which gates only the selection shortcuts). While
 * enabled it owns the rewrite: the AI-Correction pass is skipped even on its
 * own shortcut, so the Polish prompt is the only prompt applied to dictation.
 */
export const PolishDictation: React.FC<PolishDictationToggleProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false, icon, tone }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("polish_after_dictation") ?? false;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("polish_after_dictation", value)}
        isUpdating={isUpdating("polish_after_dictation")}
        label={t("settings.general.polishDictation.label")}
        description={t("settings.general.polishDictation.description")}
        info={t("settings.general.polishDictation.info")}
        icon={icon}
        tone={tone}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
