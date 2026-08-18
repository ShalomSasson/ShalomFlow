import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AppWindow, Monitor } from "lucide-react";
import { commands, type InsightsDay, type InsightsStats } from "@/bindings";
import { SectionHeader } from "../../ui/SectionHeader";

/** Reference speaking pace the WPM gauge is measured against. */
const WPM_TARGET = 140;

/** Heatmap geometry: one column per week, one row per weekday. */
const WEEKS = 17;
const CELL = 12;
const GAP = 3;
const TOOLTIP_WIDTH = 230;

/** Local-timezone YYYY-MM-DD, matching what the backend stores. */
const localDayString = (date: Date): string => {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
};

/** Bar fills fade with rank so the top app reads strongest (reference:
 *  the App usage card's opacity ladder). Static classes for Tailwind. */
const BAR_TONES = [
  "bg-accent",
  "bg-accent/75",
  "bg-accent/60",
  "bg-accent/50",
  "bg-accent/40",
  "bg-accent/35",
];

/** Dictations-per-day → heatmap intensity (reference thresholds). */
const heatmapCellClass = (count: number): string => {
  if (count === 0) return "bg-ink/6";
  if (count === 1) return "bg-accent/25";
  if (count === 2) return "bg-accent/45";
  if (count <= 4) return "bg-accent/70";
  return "bg-accent";
};

/** Semicircle gauge for the words-per-minute card. */
const WPMGauge: React.FC<{ progress: number; label: string }> = ({
  progress,
  label,
}) => {
  const width = 190;
  const stroke = 14;
  const radius = (width - stroke) / 2;
  const cx = width / 2;
  const cy = width / 2;
  const fraction = Math.min(1, Math.max(0.02, progress));
  // Sweep from 180° (left) clockwise over the top; y grows downward in SVG.
  const angle = Math.PI * (1 - fraction);
  const endX = cx + radius * Math.cos(angle);
  const endY = cy - radius * Math.sin(angle);
  const track = `M ${cx - radius} ${cy} A ${radius} ${radius} 0 0 1 ${cx + radius} ${cy}`;
  const arc = `M ${cx - radius} ${cy} A ${radius} ${radius} 0 0 1 ${endX} ${endY}`;

  return (
    <div
      className="relative mx-auto"
      style={{ width, height: cy + stroke / 2 }}
    >
      <svg
        width={width}
        height={cy + stroke / 2}
        viewBox={`0 0 ${width} ${cy + stroke / 2}`}
        aria-hidden="true"
      >
        <path
          d={track}
          fill="none"
          stroke="currentColor"
          strokeWidth={stroke}
          strokeLinecap="round"
          className="text-ink/8"
        />
        <path
          d={arc}
          fill="none"
          stroke="currentColor"
          strokeWidth={stroke}
          strokeLinecap="round"
          className="text-accent"
        />
      </svg>
      <span className="absolute bottom-0 inset-x-0 text-center text-[13px] text-muted">
        {label}
      </span>
    </div>
  );
};

/** GitHub-style dictations-per-day heatmap covering the most recent
 *  17 weeks. Hovering a day shows a tooltip with that day's totals. */
const StreakHeatmap: React.FC<{ daily: InsightsDay[] }> = ({ daily }) => {
  const { t, i18n } = useTranslation();
  const [hovered, setHovered] = useState<{
    day: Date;
    column: number;
    row: number;
  } | null>(null);

  const byDay = useMemo(() => {
    const map = new Map<string, InsightsDay>();
    for (const entry of daily) map.set(entry.day, entry);
    return map;
  }, [daily]);

  const numberFormat = useMemo(
    () => new Intl.NumberFormat(i18n.language),
    [i18n.language],
  );

  const today = new Date();
  today.setHours(0, 0, 0, 0);
  // Columns are Sunday-started weeks, ending with the current one.
  const currentWeekStart = new Date(today);
  currentWeekStart.setDate(today.getDate() - today.getDay());
  const weekStarts = Array.from({ length: WEEKS }, (_, index) => {
    const start = new Date(currentWeekStart);
    start.setDate(currentWeekStart.getDate() - (WEEKS - 1 - index) * 7);
    return start;
  });

  const weekdayFormat = new Intl.DateTimeFormat(i18n.language, {
    weekday: "short",
  });
  const monthFormat = new Intl.DateTimeFormat(i18n.language, {
    month: "short",
  });

  const cellDate = (weekStart: Date, row: number): Date => {
    const date = new Date(weekStart);
    date.setDate(weekStart.getDate() + row);
    return date;
  };

  const tooltip = () => {
    if (!hovered) return null;
    const insight = byDay.get(localDayString(hovered.day));
    const cellX = hovered.column * (CELL + GAP);
    const cellY = 12 + GAP + hovered.row * (CELL + GAP);
    const gridWidth = WEEKS * (CELL + GAP) - GAP;
    const x = Math.min(
      Math.max(0, cellX - TOOLTIP_WIDTH / 2 + CELL / 2),
      gridWidth - TOOLTIP_WIDTH,
    );
    // Flips below the cell when the top rows would push it out of the card.
    const showBelow = hovered.row < 3;
    const position: React.CSSProperties = {
      width: TOOLTIP_WIDTH,
      insetInlineStart: x,
    };
    if (showBelow) {
      position.top = cellY + CELL + 6;
    } else {
      position.bottom = `calc(100% - ${cellY - 6}px)`;
    }
    const dateFormat = new Intl.DateTimeFormat(i18n.language, {
      dateStyle: "long",
    });
    const rows: [string, string][] = [
      [
        t("settings.insights.tooltipDictations"),
        numberFormat.format(insight?.dictations ?? 0),
      ],
      [
        t("settings.insights.tooltipWords"),
        numberFormat.format(insight?.words ?? 0),
      ],
      [
        t("settings.insights.tooltipApps"),
        numberFormat.format(insight?.apps ?? 0),
      ],
      [t("settings.insights.tooltipTopApp"), insight?.top_app ?? "—"],
    ];
    return (
      <div
        className="pointer-events-none absolute z-10 rounded-xl border border-hairline-strong bg-surface p-3.5 shadow-lg"
        style={position}
      >
        <div className="mb-2 text-[13px] font-semibold text-ink">
          {dateFormat.format(hovered.day)}
        </div>
        <div className="space-y-1.5">
          {rows.map(([label, value]) => (
            <div key={label} className="flex items-center justify-between">
              <span className="text-xs text-muted">{label}</span>
              <span className="text-xs font-semibold text-ink">{value}</span>
            </div>
          ))}
        </div>
      </div>
    );
  };

  return (
    <div className="flex items-start gap-2">
      {/* Weekday labels */}
      <div className="flex flex-col" style={{ gap: GAP }}>
        <div style={{ height: 12 }} />
        {Array.from({ length: 7 }, (_, row) => (
          <div
            key={row}
            className="text-[9px] leading-none text-muted-soft"
            style={{ height: CELL, width: 26 }}
          >
            {weekdayFormat.format(cellDate(currentWeekStart, row))}
          </div>
        ))}
      </div>
      <div className="relative">
        {/* Month labels, shown at the week where the month changes */}
        <div className="flex" style={{ gap: GAP, height: 12 }}>
          {weekStarts.map((weekStart, index) => {
            const showLabel =
              index === 0 ||
              weekStarts[index - 1].getMonth() !== weekStart.getMonth();
            return (
              <div
                key={index}
                className="overflow-visible whitespace-nowrap text-[9px] leading-none text-muted-soft"
                style={{ width: CELL, marginTop: 0 }}
              >
                {showLabel ? monthFormat.format(weekStart) : ""}
              </div>
            );
          })}
        </div>
        <div className="mt-[3px] flex flex-col" style={{ gap: GAP }}>
          {Array.from({ length: 7 }, (_, row) => (
            <div key={row} className="flex" style={{ gap: GAP }}>
              {weekStarts.map((weekStart, column) => {
                const day = cellDate(weekStart, row);
                if (day > today) {
                  return (
                    <div key={column} style={{ width: CELL, height: CELL }} />
                  );
                }
                const count = byDay.get(localDayString(day))?.dictations ?? 0;
                return (
                  <div
                    key={column}
                    className={`rounded-[3px] ${heatmapCellClass(count)}`}
                    style={{ width: CELL, height: CELL }}
                    onMouseEnter={() => setHovered({ day, column, row })}
                    onMouseLeave={() => setHovered(null)}
                  />
                );
              })}
            </div>
          ))}
        </div>
        {tooltip()}
      </div>
    </div>
  );
};

/** Usage analytics page: everything is computed locally from the dictation
 *  aggregates on this device — no cloud, no sharing. */
export const InsightsSettings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const [stats, setStats] = useState<InsightsStats | null>(null);
  /** App-usage row whose words popup is showing, keyed by app name. */
  const [hoveredApp, setHoveredApp] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void commands.getInsightsStats().then((result) => {
      if (!cancelled && result.status === "ok") setStats(result.data);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const numberFormat = useMemo(
    () => new Intl.NumberFormat(i18n.language),
    [i18n.language],
  );

  const caption = (text: string) => (
    <div className="text-[11px] font-medium uppercase tracking-widest text-muted-soft">
      {text}
    </div>
  );

  const statCard = (children: React.ReactNode) => (
    <div className="flex min-h-[190px] flex-col gap-1.5 rounded-2xl border border-hairline bg-surface p-5">
      {children}
    </div>
  );

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <SectionHeader
        title={t("sidebar.insights")}
        description={t("sectionSubtitles.insights")}
      />
      {stats && (
        <>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
            {/* Words per minute */}
            {statCard(
              <>
                <div className="font-display text-3xl leading-none text-ink">
                  {numberFormat.format(stats.average_wpm)}
                </div>
                {caption(t("settings.insights.wpmCaption"))}
                <div className="pt-3">
                  <WPMGauge
                    progress={stats.average_wpm / WPM_TARGET}
                    label={t("settings.insights.wpmTarget", {
                      target: WPM_TARGET,
                    })}
                  />
                </div>
              </>,
            )}
            {/* Fixes made by AI */}
            {statCard(
              <>
                <div className="font-display text-3xl leading-none text-ink">
                  {numberFormat.format(stats.words_changed)}
                </div>
                {caption(t("settings.insights.fixesCaption"))}
                <div className="my-2.5 border-t border-hairline" />
                <div className="text-[13px] text-body">
                  {t("settings.insights.transcriptsCleaned", {
                    value: numberFormat.format(stats.cleaned_transcripts),
                  })}
                </div>
                <div className="text-[13px] text-body">
                  {t("settings.insights.wordsChanged", {
                    value: numberFormat.format(stats.words_changed),
                  })}
                </div>
              </>,
            )}
            {/* Total words dictated */}
            {statCard(
              <>
                <div className="flex items-start justify-between gap-2">
                  <div className="font-display text-3xl leading-none text-ink">
                    {numberFormat.format(stats.total_words)}
                  </div>
                  {stats.month_change_percent !== null && (
                    <span className="rounded-full bg-accent/12 px-2 py-1 text-[11px] font-semibold leading-none text-accent">
                      {`${stats.month_change_percent >= 0 ? "↗" : "↘"} ${t(
                        "settings.insights.monthChange",
                        {
                          value: numberFormat.format(
                            Math.abs(stats.month_change_percent),
                          ),
                        },
                      )}`}
                    </span>
                  )}
                </div>
                {caption(t("settings.insights.totalWordsCaption"))}
                <div className="my-2.5 border-t border-hairline" />
                <div className="flex items-center gap-2 text-[13px] text-body">
                  <Monitor className="h-3.5 w-3.5 shrink-0 text-muted" />
                  {t("settings.insights.wordsThisMonth", {
                    value: numberFormat.format(stats.words_this_month),
                  })}
                </div>
              </>,
            )}
          </div>

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            {/* App usage */}
            <div className="flex min-h-[320px] flex-col gap-4 rounded-2xl border border-hairline bg-surface p-5">
              <div className="flex items-baseline justify-between gap-2">
                <h2 className="font-display text-lg text-ink">
                  {t("settings.insights.appUsage")}
                </h2>
                {caption(
                  t("settings.insights.totalApps", {
                    value: numberFormat.format(stats.total_apps),
                  }),
                )}
              </div>
              {stats.app_usage.length === 0 ? (
                <div className="flex flex-1 items-center justify-center text-sm text-muted">
                  {t("settings.insights.empty")}
                </div>
              ) : (
                <div className="space-y-3">
                  {stats.app_usage.slice(0, 6).map((usage, rank) => {
                    const maxCount = stats.app_usage[0]?.count ?? 1;
                    const fraction = usage.count / Math.max(1, maxCount);
                    const name =
                      usage.name === ""
                        ? t("settings.insights.otherApp")
                        : usage.name;
                    return (
                      <div key={usage.name} className="flex items-center gap-3">
                        <AppWindow className="h-4 w-4 shrink-0 text-muted" />
                        <div className="flex min-w-0 flex-1 items-center gap-3">
                          <div
                            className="relative h-[26px] shrink-0"
                            style={{
                              width: `${Math.max(14, Math.round(fraction * 55))}%`,
                            }}
                            onMouseEnter={() => setHoveredApp(usage.name)}
                            onMouseLeave={() => setHoveredApp(null)}
                          >
                            <div
                              className={`flex h-full items-center justify-center rounded-md ${
                                BAR_TONES[Math.min(rank, BAR_TONES.length - 1)]
                              }`}
                            >
                              <span className="text-[11px] font-semibold text-on-primary">
                                {`${usage.percent}%`}
                              </span>
                            </div>
                            {hoveredApp === usage.name && (
                              <div className="pointer-events-none absolute bottom-full left-1/2 z-10 mb-2 -translate-x-1/2">
                                <div className="rounded-xl bg-ink px-3.5 py-2.5 shadow-lg">
                                  <div className="whitespace-nowrap text-[13px] font-semibold text-surface">
                                    {`🚀 ${t("settings.insights.powerUsage")}`}
                                  </div>
                                  <div className="whitespace-nowrap text-xs text-surface/80">
                                    {t("settings.insights.powerUsageWords", {
                                      value: numberFormat.format(usage.words),
                                    })}
                                  </div>
                                </div>
                                {/* Caret pointing back down at the bar. */}
                                <div
                                  className="mx-auto h-2 w-3 bg-ink"
                                  style={{
                                    clipPath: "polygon(0 0, 100% 0, 50% 100%)",
                                  }}
                                />
                              </div>
                            )}
                          </div>
                          <span className="truncate text-[11px] font-medium uppercase tracking-wide text-muted">
                            {`${numberFormat.format(usage.count)} · ${name}`}
                          </span>
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
            {/* Streak */}
            <div className="flex min-h-[320px] flex-col gap-4 rounded-2xl border border-hairline bg-surface p-5">
              <div className="flex items-baseline justify-between gap-2">
                <h2 className="font-display text-lg text-ink">
                  {t("settings.insights.dayStreak", {
                    value: numberFormat.format(stats.day_streak),
                  })}
                </h2>
                {caption(
                  t("settings.insights.longestStreak", {
                    value: numberFormat.format(stats.longest_streak),
                  }),
                )}
              </div>
              <StreakHeatmap daily={stats.daily} />
              <div className="mt-auto flex items-center gap-1.5">
                <span className="text-[11px] text-muted">
                  {t("settings.insights.more")}
                </span>
                {[
                  "bg-accent",
                  "bg-accent/70",
                  "bg-accent/45",
                  "bg-accent/25",
                ].map((tone) => (
                  <span
                    key={tone}
                    className={`h-3 w-3 rounded-[3px] ${tone}`}
                  />
                ))}
                <span className="text-[11px] text-muted">
                  {t("settings.insights.less")}
                </span>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
};
