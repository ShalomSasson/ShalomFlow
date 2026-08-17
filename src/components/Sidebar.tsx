import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  SlidersHorizontal,
  Mic,
  ChartColumn,
  ChevronDown,
  FlaskConical,
  History,
  Info,
  MessageCircle,
  ChevronLeft,
  ChevronRight,
  Settings,
  Type,
  Wand2,
} from "lucide-react";
import { useSettings } from "../hooks/useSettings";
import {
  GeneralSettings,
  DictationSettings,
  HistorySettings,
  InsightsSettings,
  DebugSettings,
  AboutSettings,
  AssistantSection,
  StylesSettings,
  TransformsSettings,
} from "./settings";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: any;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
  enabled: (settings: any) => boolean;
}

// Section registry (keys are stable so `t()` call sites and code don't
// churn). The old Models / Advanced / Post Process / Profiles / Memory
// sections were folded in: Models + Post Process live inside Dictation;
// Profiles + Memory are sub-pages of Assistant (see AssistantSection);
// Advanced's rows moved into General (a "More options" fold) and History
// (retention fold). The rail's visual order and nesting live in
// SIDEBAR_LAYOUT below, not in this map.
export const SECTIONS_CONFIG = {
  insights: {
    labelKey: "sidebar.insights",
    icon: ChartColumn,
    component: InsightsSettings,
    enabled: () => true,
  },
  history: {
    labelKey: "sidebar.history",
    icon: History,
    component: HistorySettings,
    enabled: () => true,
  },
  styles: {
    labelKey: "sidebar.styles",
    icon: Type,
    component: StylesSettings,
    enabled: () => true,
  },
  transforms: {
    labelKey: "sidebar.transforms",
    icon: Wand2,
    component: TransformsSettings,
    enabled: () => true,
  },
  general: {
    labelKey: "sidebar.general",
    icon: SlidersHorizontal,
    component: GeneralSettings,
    enabled: () => true,
  },
  dictation: {
    labelKey: "sidebar.dictation",
    icon: Mic,
    component: DictationSettings,
    enabled: () => true,
  },
  assistant: {
    labelKey: "sidebar.assistant",
    icon: MessageCircle,
    component: AssistantSection,
    enabled: () => true,
  },
  debug: {
    labelKey: "sidebar.debug",
    icon: FlaskConical,
    component: DebugSettings,
    enabled: (settings) => settings?.debug_mode ?? false,
  },
  about: {
    labelKey: "sidebar.about",
    icon: Info,
    component: AboutSettings,
    enabled: () => true,
  },
} as const satisfies Record<string, SectionConfig>;

/** Sections grouped under the expandable "Settings" parent row. */
const SETTINGS_GROUP: SidebarSection[] = [
  "general",
  "dictation",
  "assistant",
  "debug",
];

/** Top-level rail order: standalone sections, with the Settings group
 *  rendered as one expandable parent between Prompts and About. */
const TOP_SECTIONS: SidebarSection[] = [
  "insights",
  "history",
  "styles",
  "transforms",
];

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  // The sidebar always opens expanded; the toggle only affects the current
  // session (no persistence), so the full rail is the default on every launch.
  const [collapsed, setCollapsed] = useState(false);
  const activeIsSettingsChild = SETTINGS_GROUP.includes(activeSection);
  // The Settings group opens when a child is active and stays user-controlled
  // otherwise; navigating into a child (e.g. via first launch) re-opens it.
  const [settingsOpen, setSettingsOpen] = useState(activeIsSettingsChild);
  useEffect(() => {
    if (activeIsSettingsChild) setSettingsOpen(true);
  }, [activeIsSettingsChild]);

  const isEnabled = (id: SidebarSection) =>
    SECTIONS_CONFIG[id].enabled(settings);
  const settingsChildren = SETTINGS_GROUP.filter(isEnabled);

  const toggleLabel = collapsed ? t("sidebar.expand") : t("sidebar.collapse");

  const sectionButton = (id: SidebarSection, indent = false) => {
    const config = SECTIONS_CONFIG[id];
    const Icon = config.icon;
    const isActive = activeSection === id;
    return (
      <button
        key={id}
        type="button"
        aria-current={isActive ? "page" : undefined}
        title={t(config.labelKey)}
        className={`group flex gap-2.5 items-center h-[34px] w-full rounded-lg cursor-pointer transition-colors duration-150 text-start ${
          collapsed ? "justify-center px-0" : indent ? "ps-8 pe-2.5" : "px-2.5"
        } ${
          isActive
            ? "bg-accent/12 text-accent font-medium"
            : "text-body font-normal hover:text-ink hover:bg-ink/6"
        }`}
        onClick={() => onSectionChange(id)}
      >
        <Icon
          width={16}
          height={16}
          className={`shrink-0 transition-opacity ${
            isActive ? "opacity-100" : "opacity-70 group-hover:opacity-100"
          }`}
        />
        {!collapsed && (
          <span className="text-[13px] truncate">{t(config.labelKey)}</span>
        )}
      </button>
    );
  };

  return (
    <div
      className={`flex flex-col h-full bg-canvas-soft pt-4 pb-3 overflow-hidden transition-[width] duration-200 ease-out motion-reduce:transition-none ${
        collapsed ? "w-16 items-center px-2" : "w-52 px-3"
      }`}
    >
      {/* Brand lives in the TitleBar now; the rail is pure navigation. */}
      <nav className="flex flex-col w-full gap-px">
        {TOP_SECTIONS.filter(isEnabled).map((id) => sectionButton(id))}
        {/* Settings parent row. The collapsed icon rail has no room for
            hierarchy, so it flattens the group into plain icon buttons. */}
        {collapsed ? (
          settingsChildren.map((id) => sectionButton(id))
        ) : (
          <>
            <button
              type="button"
              aria-expanded={settingsOpen}
              title={t("sidebar.settings")}
              className={`group flex gap-2.5 items-center h-[34px] w-full rounded-lg cursor-pointer transition-colors duration-150 text-start px-2.5 ${
                activeIsSettingsChild && !settingsOpen
                  ? "bg-accent/12 text-accent font-medium"
                  : "text-body font-normal hover:text-ink hover:bg-ink/6"
              }`}
              onClick={() => {
                // First click opens the group and lands on General; a click
                // while open just folds it back up.
                if (!settingsOpen && !activeIsSettingsChild) {
                  onSectionChange("general");
                }
                setSettingsOpen((open) => !open);
              }}
            >
              <Settings
                width={16}
                height={16}
                className="shrink-0 opacity-70 transition-opacity group-hover:opacity-100"
              />
              <span className="text-[13px] truncate flex-1">
                {t("sidebar.settings")}
              </span>
              <ChevronDown
                width={14}
                height={14}
                className={`shrink-0 opacity-50 transition-transform ${
                  settingsOpen ? "" : "-rotate-90 rtl:rotate-90"
                }`}
              />
            </button>
            {settingsOpen &&
              settingsChildren.map((id) => sectionButton(id, true))}
          </>
        )}
        {sectionButton("about")}
      </nav>

      {/* Collapse control — a plain chevron pinned to the bottom, kept clear of
          the logo and nav. Points inward (left) to collapse, outward (right) to
          expand. */}
      <button
        type="button"
        onClick={() => setCollapsed((value) => !value)}
        aria-label={toggleLabel}
        aria-expanded={!collapsed}
        title={toggleLabel}
        className={`mt-auto flex items-center h-8 w-full rounded-lg cursor-pointer text-muted-soft transition-colors hover:text-ink hover:bg-ink/4 ${
          collapsed ? "justify-center" : "justify-start px-2.5"
        }`}
      >
        {collapsed ? (
          <ChevronRight width={16} height={16} />
        ) : (
          <ChevronLeft width={16} height={16} />
        )}
      </button>
    </div>
  );
};
