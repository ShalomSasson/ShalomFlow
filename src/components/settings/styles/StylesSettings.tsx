/* eslint-disable i18next/no-literal-string -- the card sample previews
 * (chat messages, email snippets, avatar initials, timestamps) demonstrate
 * ENGLISH formatting output and are deliberately untranslated: the Style
 * feature itself only formats English text (see the promo banner copy). */
import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import Badge from "../../ui/Badge";
import { SectionHeader } from "../../ui/SectionHeader";
import { useSettings } from "../../../hooks/useSettings";
import type { StylePrefs } from "@/bindings";

type StyleContext = keyof StylePrefs;

interface ChatSample {
  kind: "chat";
  avatar: string;
  avatarClass: string;
  name: string;
  time: string;
  body: string;
}
interface EmailSample {
  kind: "email";
  to: string;
  body: string;
}
interface TextSample {
  kind: "text";
  body: string;
}
type StyleSample = ChatSample | EmailSample | TextSample;

interface StyleCardSpec {
  /** Persisted preset key (stored in settings.style_prefs). */
  id: string;
  /** i18n key suffix for the preset name + rule line. */
  nameKey: string;
  ruleKey: string;
  sample: StyleSample;
}

interface StyleTabSpec {
  id: StyleContext;
  labelKey: string;
  beta?: boolean;
  promoTitleKey: string;
  promoIcons: { label: string; className: string }[];
  cards: StyleCardSpec[];
}

const chat = (
  avatar: string,
  avatarClass: string,
  body: string,
): ChatSample => ({
  kind: "chat",
  avatar,
  avatarClass,
  name: "John Doe",
  time: "9:45 AM",
  body,
});

// Static presentation data mirroring the reference Style page. Selection is
// real: it persists into settings.style_prefs.
const STYLE_TABS: StyleTabSpec[] = [
  {
    id: "personal",
    labelKey: "styles.tabs.personal",
    promoTitleKey: "styles.promo.personal",
    promoIcons: [
      { label: "WA", className: "bg-[#22C55E]" },
      { label: "TG", className: "bg-[#0EA5E9]" },
      { label: "DC", className: "bg-[#A855F7]" },
      { label: "IG", className: "bg-[#F472B6]" },
      { label: "+", className: "bg-[#1F2937]" },
    ],
    cards: [
      {
        id: "formal",
        nameKey: "styles.presets.formal",
        ruleKey: "styles.rules.capsPunctuation",
        sample: chat(
          "J",
          "bg-[#8b6fd0]",
          "Hey, are you free for lunch tomorrow? Let's do 12 if that works for you.",
        ),
      },
      {
        id: "casual",
        nameKey: "styles.presets.casual",
        ruleKey: "styles.rules.capsLessPunctuation",
        sample: chat(
          "J",
          "bg-[#e084b8]",
          "Hey are you free for lunch tomorrow? let's do 12 if that works for you",
        ),
      },
      {
        id: "very_casual",
        nameKey: "styles.presets.veryCasual",
        ruleKey: "styles.rules.noCapsLessPunctuation",
        sample: chat(
          "J",
          "bg-[#6E2EB5]",
          "hey are you free for lunch tomorrow? let's do 12 if that works for you",
        ),
      },
    ],
  },
  {
    id: "work",
    labelKey: "styles.tabs.work",
    promoTitleKey: "styles.promo.work",
    promoIcons: [
      { label: "SL", className: "bg-[#4A154B]" },
      { label: "TM", className: "bg-[#4F46E5]" },
      { label: "in", className: "bg-[#0EA5E9]" },
      { label: "+", className: "bg-[#1F2937]" },
    ],
    cards: [
      {
        id: "formal",
        nameKey: "styles.presets.formal",
        ruleKey: "styles.rules.capsPunctuation",
        sample: chat(
          "J",
          "bg-[#8b6fd0]",
          "Hey, if you're free, let's chat about the great results.",
        ),
      },
      {
        id: "casual",
        nameKey: "styles.presets.casual",
        ruleKey: "styles.rules.capsLessPunctuation",
        sample: chat(
          "J",
          "bg-[#e084b8]",
          "Hey, if you're free let's chat about the great results",
        ),
      },
      {
        id: "excited",
        nameKey: "styles.presets.excited",
        ruleKey: "styles.rules.moreExclamations",
        sample: chat(
          "J",
          "bg-[#6E2EB5]",
          "Hey, if you're free, let's chat about the great results!",
        ),
      },
    ],
  },
  {
    id: "email",
    labelKey: "styles.tabs.email",
    promoTitleKey: "styles.promo.email",
    promoIcons: [
      { label: "G", className: "bg-[#DC2626]" },
      { label: "O", className: "bg-[#0EA5E9]" },
      { label: "Y", className: "bg-[#A855F7]" },
      { label: "+", className: "bg-[#1F2937]" },
    ],
    cards: [
      {
        id: "formal",
        nameKey: "styles.presets.formal",
        ruleKey: "styles.rules.capsPunctuation",
        sample: {
          kind: "email",
          to: "Alex Doe",
          body: "Hi Alex,\n\nIt was great talking with you today. Looking forward to our next chat.\n\nBest,\nMary",
        },
      },
      {
        id: "casual",
        nameKey: "styles.presets.casual",
        ruleKey: "styles.rules.capsLessPunctuation",
        sample: {
          kind: "email",
          to: "Alex Doe",
          body: "Hi Alex, it was great talking with you today. Looking forward to our next chat.\n\nBest,\nMary",
        },
      },
      {
        id: "excited",
        nameKey: "styles.presets.excited",
        ruleKey: "styles.rules.moreExclamations",
        sample: {
          kind: "email",
          to: "Alex Doe",
          body: "Hi Alex,\n\nIt was great talking with you today. Looking forward to our next chat!\n\nBest,\nMary",
        },
      },
    ],
  },
  {
    id: "other",
    labelKey: "styles.tabs.other",
    promoTitleKey: "styles.promo.other",
    promoIcons: [
      { label: "O", className: "bg-[#22C55E]" },
      { label: "AI", className: "bg-[#0EA5E9]" },
      { label: "N", className: "bg-[#F2C94C]" },
      { label: "+", className: "bg-[#1F2937]" },
    ],
    cards: [
      {
        id: "formal",
        nameKey: "styles.presets.formal",
        ruleKey: "styles.rules.capsPunctuation",
        sample: {
          kind: "text",
          body: "So far, I am enjoying the new workout routine.\n\nI am excited for tomorrow's workout, especially after a full night of rest.",
        },
      },
      {
        id: "casual",
        nameKey: "styles.presets.casual",
        ruleKey: "styles.rules.capsLessPunctuation",
        sample: {
          kind: "text",
          body: "So far I am enjoying the new workout routine.\n\nI am excited for tomorrow's workout especially after a full night of rest.",
        },
      },
      {
        id: "excited",
        nameKey: "styles.presets.excited",
        ruleKey: "styles.rules.moreExclamations",
        sample: {
          kind: "text",
          body: "So far, I am enjoying the new workout routine!\n\nI am excited for tomorrow's workout, especially after a full night of rest!",
        },
      },
    ],
  },
  {
    id: "auto_cleanup",
    labelKey: "styles.tabs.autoCleanup",
    beta: true,
    promoTitleKey: "styles.promo.autoCleanup",
    promoIcons: [
      { label: "✨", className: "bg-[#22C55E]" },
      { label: "🪄", className: "bg-[#0EA5E9]" },
      { label: "🧹", className: "bg-[#A855F7]" },
    ],
    cards: [
      {
        id: "off",
        nameKey: "styles.presets.off",
        ruleKey: "styles.rules.keepRaw",
        sample: {
          kind: "text",
          body: "Um so, I, I was thinking, ah, that we could, you know, ship the feature tomorrow if that works.",
        },
      },
      {
        id: "light",
        nameKey: "styles.presets.light",
        ruleKey: "styles.rules.trimFillers",
        sample: {
          kind: "text",
          body: "So I was thinking that we could ship the feature tomorrow if that works.",
        },
      },
      {
        id: "aggressive",
        nameKey: "styles.presets.aggressive",
        ruleKey: "styles.rules.trimFillersTighten",
        sample: {
          kind: "text",
          body: "I think we should ship the feature tomorrow.",
        },
      },
    ],
  },
];

const SampleView: React.FC<{ sample: StyleSample }> = ({ sample }) => {
  switch (sample.kind) {
    case "chat":
      return (
        <div className="flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <span
              className={`flex items-center justify-center w-6 h-6 rounded-md text-[11px] font-bold text-white ${sample.avatarClass}`}
            >
              {sample.avatar}
            </span>
            <span className="text-xs font-semibold text-ink">
              {sample.name}
            </span>
            <span className="text-[11px] text-muted-soft">{sample.time}</span>
          </div>
          <p className="text-xs leading-relaxed text-ink bg-surface-strong rounded-lg p-2.5">
            {sample.body}
          </p>
        </div>
      );
    case "email":
      return (
        <div className="flex flex-col gap-1.5">
          <span className="text-[11px] text-muted-soft">
            To: {sample.to}
          </span>
          <p className="text-xs leading-relaxed text-ink whitespace-pre-line">
            {sample.body}
          </p>
        </div>
      );
    case "text":
      return (
        <p className="text-xs leading-relaxed text-ink whitespace-pre-line">
          {sample.body}
        </p>
      );
  }
};

/**
 * Styles — per-context tone presets (personal / work / email / other) plus
 * Auto Cleanup strength, mirroring the reference Style page. Selections
 * persist into settings.style_prefs.
 */
export const StylesSettings: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();
  const [activeTab, setActiveTab] = useState(0);

  const tab = STYLE_TABS[activeTab];
  const prefs: StylePrefs = settings?.style_prefs ?? {
    personal: "very_casual",
    work: "casual",
    email: "casual",
    other: "casual",
    auto_cleanup: "light",
  };

  const select = (cardId: string) => {
    void updateSetting("style_prefs", { ...prefs, [tab.id]: cardId });
  };

  return (
    <div className="max-w-4xl w-full mx-auto space-y-6">
      <SectionHeader
        title={t("sidebar.styles")}
        description={t("sectionSubtitles.styles")}
      />

      {/* Context tab bar */}
      <div className="flex items-end gap-5 border-b border-hairline">
        {STYLE_TABS.map((tabSpec, index) => (
          <button
            key={tabSpec.id}
            type="button"
            className={`flex items-center gap-1.5 pb-2.5 -mb-px border-b-2 cursor-pointer text-[13px] transition-colors ${
              index === activeTab
                ? "border-ink text-ink font-semibold"
                : "border-transparent text-muted hover:text-ink"
            }`}
            onClick={() => setActiveTab(index)}
          >
            {t(tabSpec.labelKey)}
            {tabSpec.beta && (
              <Badge variant="active">{t("styles.beta")}</Badge>
            )}
          </button>
        ))}
      </div>

      {/* Promo banner */}
      {/* bg-ink/text-canvas invert automatically with the theme tokens. */}
      <div className="flex items-center justify-between gap-6 rounded-xl bg-ink text-canvas px-6 py-5">
        <div className="min-w-0">
          <h2 className="font-display text-lg leading-snug">
            {t(tab.promoTitleKey)}
          </h2>
          <p className="mt-1 text-xs opacity-75">{t("styles.promo.note")}</p>
        </div>
        <div className="flex shrink-0 -space-x-1">
          {tab.promoIcons.map((icon, i) => (
            <span
              key={i}
              className={`flex items-center justify-center w-8 h-8 rounded-lg text-[11px] font-bold text-white ring-2 ring-ink ${icon.className} ${
                i % 2 === 1 ? "translate-y-1" : ""
              }`}
            >
              {icon.label}
            </span>
          ))}
        </div>
      </div>

      {/* Preset cards */}
      <div className="grid grid-cols-3 gap-4">
        {tab.cards.map((card) => {
          const selected = prefs[tab.id] === card.id;
          return (
            <button
              key={card.id}
              type="button"
              aria-pressed={selected}
              className={`flex flex-col items-stretch gap-3 rounded-xl border bg-surface p-4 text-start cursor-pointer transition-colors min-h-[230px] ${
                selected
                  ? "border-accent ring-1 ring-accent"
                  : "border-hairline hover:border-hairline-strong"
              }`}
              onClick={() => select(card.id)}
            >
              <div>
                <h3 className="font-display text-lg text-ink">
                  {t(card.nameKey)}
                </h3>
                <p className="mt-0.5 text-xs text-muted">{t(card.ruleKey)}</p>
              </div>
              <SampleView sample={card.sample} />
            </button>
          );
        })}
      </div>
    </div>
  );
};
