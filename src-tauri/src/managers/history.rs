use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Days, Local, NaiveDate, Utc};
use log::{debug, error, info};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};
use tauri_specta::Event;

use crate::llm_client::ChatMessage;

/// Database migrations for transcription history.
/// Each migration is applied in order. The library tracks which migrations
/// have been applied using SQLite's user_version pragma.
///
/// Note: For users upgrading from tauri-plugin-sql, migrate_from_tauri_plugin_sql()
/// converts the old _sqlx_migrations table tracking to the user_version pragma,
/// ensuring migrations don't re-run on existing databases.
static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
    ),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;"),
    // Assistant conversations live alongside transcriptions but in their own
    // table: one row per conversation session, with the messages stored as a
    // JSON array. `timestamp` is when the conversation started, `updated_at`
    // is the last turn — the History view sorts by the latter so an active
    // chat stays near the top.
    M::up(
        "CREATE TABLE IF NOT EXISTS assistant_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            title TEXT NOT NULL,
            messages TEXT NOT NULL
        );",
    ),
    // Lifetime usage stats for the History page (total words, speaking time,
    // day streak). A single-row table rather than aggregating history rows:
    // recordings are pruned by the retention limit, so lifetime numbers must
    // survive their source rows. `last_active_day` is a local-timezone
    // YYYY-MM-DD string used for the streak.
    M::up(
        "CREATE TABLE IF NOT EXISTS usage_stats (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            total_words INTEGER NOT NULL DEFAULT 0,
            total_speech_ms INTEGER NOT NULL DEFAULT 0,
            streak_days INTEGER NOT NULL DEFAULT 0,
            last_active_day TEXT NOT NULL DEFAULT ''
        );",
    ),
    // Insights page: per-day, per-app dictation aggregates plus lifetime
    // AI-fix counters. daily_usage rows are never pruned by retention (unlike
    // transcription_history), so the heatmap, streaks and app-usage numbers
    // survive recording cleanup. `day` is a local-timezone YYYY-MM-DD string;
    // `app` is '' when the frontmost application could not be determined.
    M::up(
        "CREATE TABLE IF NOT EXISTS daily_usage (
            day TEXT NOT NULL,
            app TEXT NOT NULL,
            dictations INTEGER NOT NULL DEFAULT 0,
            words INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (day, app)
        );
        ALTER TABLE usage_stats ADD COLUMN cleaned_transcripts INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE usage_stats ADD COLUMN words_changed INTEGER NOT NULL DEFAULT 0;",
    ),
];

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct PaginatedHistory {
    pub entries: Vec<HistoryEntry>,
    pub has_more: bool,
}

/// A persisted assistant conversation. One row per session; `messages` is the
/// ordered turn-by-turn transcript (the same `{role, content}` shape the
/// assistant panel renders).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct AssistantHistoryEntry {
    pub id: i64,
    /// When the conversation was first saved (seconds since epoch).
    pub timestamp: i64,
    /// When the most recent turn was added (seconds since epoch).
    pub updated_at: i64,
    /// Short label derived from the first user message.
    pub title: String,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct PaginatedAssistantHistory {
    pub entries: Vec<AssistantHistoryEntry>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(tag = "action")]
pub enum HistoryUpdatePayload {
    #[serde(rename = "added")]
    Added { entry: HistoryEntry },
    #[serde(rename = "updated")]
    Updated { entry: HistoryEntry },
    #[serde(rename = "deleted")]
    Deleted { id: i64 },
    #[serde(rename = "toggled")]
    Toggled { id: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
}

/// Lifetime dictation stats shown on the History page. Independent of the
/// stored history rows so retention pruning never shrinks the numbers.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct UsageStats {
    pub total_words: i64,
    pub total_speech_ms: i64,
    pub streak_days: i64,
    /// Local-timezone YYYY-MM-DD of the last counted dictation; the frontend
    /// shows the streak as 0 when this is older than yesterday.
    pub last_active_day: String,
}

/// Dictations grouped by the application they were pasted into.
/// `name` is '' for dictations whose target app is unknown (Linux, or rows
/// recorded before app tracking existed) — the frontend labels those "Other".
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct InsightsAppUsage {
    pub name: String,
    pub count: i64,
    /// Share of all dictations, 0...100.
    pub percent: i64,
    /// Words dictated into this app.
    pub words: i64,
}

/// One calendar day's dictation activity, for the streak heatmap tooltip.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct InsightsDay {
    /// Local-timezone YYYY-MM-DD.
    pub day: String,
    pub dictations: i64,
    pub words: i64,
    /// Distinct named target apps that day.
    pub apps: i64,
    /// Most-used named target app that day, None when none was recorded.
    pub top_app: Option<String>,
}

/// Everything the Insights page shows. Computed from the pruning-proof
/// aggregates (usage_stats + daily_usage), never from the history rows.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct InsightsStats {
    pub average_wpm: i64,
    pub total_words: i64,
    pub words_this_month: i64,
    /// Words this month vs the previous month, as a percent change;
    /// None when the previous month had no dictations.
    pub month_change_percent: Option<i64>,
    /// Dictations where AI cleanup changed the raw transcript.
    pub cleaned_transcripts: i64,
    /// Words in the cleaned texts that do not appear in their raw transcripts.
    pub words_changed: i64,
    pub app_usage: Vec<InsightsAppUsage>,
    /// Distinct named target apps.
    pub total_apps: i64,
    pub day_streak: i64,
    pub longest_streak: i64,
    /// Recent per-day activity for the heatmap (oldest first).
    pub daily: Vec<InsightsDay>,
}

/// Words in `final_text` that are not present in `raw` (case-insensitive
/// multiset difference, whitespace tokens) — an approximation of how much
/// AI cleanup changed a transcript.
pub fn changed_word_count(raw: &str, final_text: &str) -> usize {
    let mut raw_words: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for word in raw.to_lowercase().split_whitespace() {
        *raw_words.entry(word.to_string()).or_insert(0) += 1;
    }
    let mut changed = 0;
    for word in final_text.to_lowercase().split_whitespace() {
        match raw_words.get_mut(word) {
            Some(remaining) if *remaining > 0 => *remaining -= 1,
            _ => changed += 1,
        }
    }
    changed
}

pub struct HistoryManager {
    app_handle: AppHandle,
    recordings_dir: PathBuf,
    db_path: PathBuf,
}

impl HistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        // Create recordings directory in app data dir
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let recordings_dir = app_data_dir.join("recordings");
        let db_path = app_data_dir.join("history.db");

        // Ensure recordings directory exists
        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            recordings_dir,
            db_path,
        };

        // Initialize database and run migrations synchronously
        manager.init_database()?;

        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);

        let mut conn = Connection::open(&self.db_path)?;

        // Handle migration from tauri-plugin-sql to rusqlite_migration
        // tauri-plugin-sql used _sqlx_migrations table, rusqlite_migration uses user_version pragma
        self.migrate_from_tauri_plugin_sql(&conn)?;

        // Create migrations object and run to latest version
        let migrations = Migrations::new(MIGRATIONS.to_vec());

        // Validate migrations in debug builds
        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid migrations");

        // Get current version before migration
        let version_before: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        debug!("Database version before migration: {}", version_before);

        // Apply any pending migrations
        migrations.to_latest(&mut conn)?;

        // Get version after migration
        let version_after: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version_after > version_before {
            info!(
                "Database migrated from version {} to {}",
                version_before, version_after
            );
        } else {
            debug!("Database already at latest version {}", version_after);
        }

        Self::backfill_daily_usage(&conn)?;

        Ok(())
    }

    /// Seed daily_usage from the history rows that survived retention, so
    /// the Insights heatmap is not empty when the feature first ships. Only
    /// runs while daily_usage is empty. Transform executions (no recording,
    /// empty file_name) are not dictations and are skipped; the target app
    /// of old rows is unknown ('').
    fn backfill_daily_usage(conn: &Connection) -> Result<()> {
        let existing: i64 = conn.query_row("SELECT COUNT(*) FROM daily_usage", [], |row| {
            row.get(0)
        })?;
        if existing > 0 {
            return Ok(());
        }

        let mut stmt = conn.prepare(
            "SELECT timestamp, transcription_text, post_processed_text
             FROM transcription_history
             WHERE file_name != ''",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (timestamp, raw, cleaned) = row?;
            let Some(utc) = DateTime::from_timestamp(timestamp, 0) else {
                continue;
            };
            let day = utc.with_timezone(&Local).format("%Y-%m-%d").to_string();
            let words = cleaned.unwrap_or(raw).split_whitespace().count() as i64;
            conn.execute(
                "INSERT INTO daily_usage (day, app, dictations, words) VALUES (?1, '', 1, ?2)
                 ON CONFLICT(day, app) DO UPDATE SET
                     dictations = dictations + 1,
                     words = words + ?2",
                params![day, words],
            )?;
        }
        Ok(())
    }

    /// Migrate from tauri-plugin-sql's migration tracking to rusqlite_migration's.
    /// tauri-plugin-sql used a _sqlx_migrations table, while rusqlite_migration uses
    /// SQLite's user_version pragma. This function checks if the old system was in use
    /// and sets the user_version accordingly so migrations don't re-run.
    fn migrate_from_tauri_plugin_sql(&self, conn: &Connection) -> Result<()> {
        // Check if the old _sqlx_migrations table exists
        let has_sqlx_migrations: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_sqlx_migrations {
            return Ok(());
        }

        // Check current user_version
        let current_version: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version > 0 {
            // Already migrated to rusqlite_migration system
            return Ok(());
        }

        // Get the highest version from the old migrations table
        let old_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if old_version > 0 {
            info!(
                "Migrating from tauri-plugin-sql (version {}) to rusqlite_migration",
                old_version
            );

            // Set user_version to match the old migration state
            conn.pragma_update(None, "user_version", old_version)?;

            // Optionally drop the old migrations table (keeping it doesn't hurt)
            // conn.execute("DROP TABLE IF EXISTS _sqlx_migrations", [])?;

            info!(
                "Migration tracking converted: user_version set to {}",
                old_version
            );
        }

        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    fn map_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
        Ok(HistoryEntry {
            id: row.get("id")?,
            file_name: row.get("file_name")?,
            timestamp: row.get("timestamp")?,
            saved: row.get("saved")?,
            title: row.get("title")?,
            transcription_text: row.get("transcription_text")?,
            post_processed_text: row.get("post_processed_text")?,
            post_process_prompt: row.get("post_process_prompt")?,
            post_process_requested: row.get("post_process_requested")?,
        })
    }

    pub fn recordings_dir(&self) -> &std::path::Path {
        &self.recordings_dir
    }

    /// Save a new history entry to the database.
    /// The WAV file should already have been written to the recordings directory.
    /// Add one dictation's words and speech time to the lifetime stats and
    /// advance the day streak (same day: unchanged; consecutive day: +1;
    /// gap: reset to 1). Days use the machine's local timezone. `app` is the
    /// frontmost application receiving the dictation (None when unknown),
    /// aggregated into daily_usage for the Insights page.
    pub fn record_usage(&self, words: i64, speech_ms: i64, app: Option<&str>) -> Result<()> {
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let yesterday = (now - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();

        let conn = self.get_connection()?;
        conn.execute("INSERT OR IGNORE INTO usage_stats (id) VALUES (1)", [])?;
        let (streak, last_day): (i64, String) = conn.query_row(
            "SELECT streak_days, last_active_day FROM usage_stats WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let new_streak = if last_day == today {
            streak.max(1)
        } else if last_day == yesterday {
            streak + 1
        } else {
            1
        };
        conn.execute(
            "UPDATE usage_stats
             SET total_words = total_words + ?1,
                 total_speech_ms = total_speech_ms + ?2,
                 streak_days = ?3,
                 last_active_day = ?4
             WHERE id = 1",
            params![words, speech_ms, new_streak, &today],
        )?;
        conn.execute(
            "INSERT INTO daily_usage (day, app, dictations, words) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(day, app) DO UPDATE SET
                 dictations = dictations + 1,
                 words = words + ?3",
            params![&today, app.unwrap_or(""), words],
        )?;
        Ok(())
    }

    /// Count one AI-cleaned transcript and how many words the cleanup changed.
    /// Lifetime counters on usage_stats — history rows are pruned by
    /// retention, so the Insights numbers cannot be derived from them.
    pub fn record_ai_fix(&self, words_changed: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("INSERT OR IGNORE INTO usage_stats (id) VALUES (1)", [])?;
        conn.execute(
            "UPDATE usage_stats
             SET cleaned_transcripts = cleaned_transcripts + 1,
                 words_changed = words_changed + ?1
             WHERE id = 1",
            params![words_changed],
        )?;
        Ok(())
    }

    pub fn get_usage_stats(&self) -> Result<UsageStats> {
        let conn = self.get_connection()?;
        conn.execute("INSERT OR IGNORE INTO usage_stats (id) VALUES (1)", [])?;
        let stats = conn.query_row(
            "SELECT total_words, total_speech_ms, streak_days, last_active_day
             FROM usage_stats WHERE id = 1",
            [],
            |row| {
                Ok(UsageStats {
                    total_words: row.get(0)?,
                    total_speech_ms: row.get(1)?,
                    streak_days: row.get(2)?,
                    last_active_day: row.get(3)?,
                })
            },
        )?;
        Ok(stats)
    }

    /// Everything the Insights page shows, computed from the pruning-proof
    /// aggregate tables (usage_stats + daily_usage) in one pass.
    pub fn get_insights(&self) -> Result<InsightsStats> {
        let conn = self.get_connection()?;
        Self::insights_with_conn(&conn, Local::now().date_naive())
    }

    fn insights_with_conn(conn: &Connection, today: NaiveDate) -> Result<InsightsStats> {
        conn.execute("INSERT OR IGNORE INTO usage_stats (id) VALUES (1)", [])?;
        let (total_words, total_speech_ms, streak_days, last_active_day, cleaned, words_changed): (
            i64,
            i64,
            i64,
            String,
            i64,
            i64,
        ) = conn.query_row(
            "SELECT total_words, total_speech_ms, streak_days, last_active_day,
                    cleaned_transcripts, words_changed
             FROM usage_stats WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;

        let average_wpm = if total_speech_ms > 0 {
            ((total_words as f64) / (total_speech_ms as f64 / 60_000.0)).round() as i64
        } else {
            0
        };

        // Words this calendar month vs the previous one, from daily_usage.
        let this_month_prefix = today.format("%Y-%m-").to_string();
        let prev_month_prefix = today
            .with_day(1)
            .and_then(|first| first.pred_opt())
            .map(|last_of_prev| last_of_prev.format("%Y-%m-").to_string());
        let month_words = |prefix: &str| -> Result<i64> {
            Ok(conn.query_row(
                "SELECT COALESCE(SUM(words), 0) FROM daily_usage WHERE day LIKE ?1 || '%'",
                params![prefix],
                |row| row.get(0),
            )?)
        };
        let words_this_month = month_words(&this_month_prefix)?;
        let words_prev_month = match &prev_month_prefix {
            Some(prefix) => month_words(prefix)?,
            None => 0,
        };
        let month_change_percent = if words_prev_month > 0 {
            Some(
                (((words_this_month - words_prev_month) as f64) / (words_prev_month as f64)
                    * 100.0)
                    .round() as i64,
            )
        } else {
            None
        };

        // Per-app usage across all recorded days.
        let mut stmt = conn.prepare(
            "SELECT app, SUM(dictations) AS count, SUM(words) AS words
             FROM daily_usage GROUP BY app ORDER BY count DESC, app ASC",
        )?;
        let by_app = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let total_dictations: i64 = by_app.iter().map(|(_, count, _)| count).sum();
        let app_usage = by_app
            .iter()
            .map(|(name, count, words)| InsightsAppUsage {
                name: name.clone(),
                count: *count,
                percent: ((*count as f64) / (total_dictations.max(1) as f64) * 100.0).round()
                    as i64,
                words: *words,
            })
            .collect();
        let total_apps = by_app
            .iter()
            .filter(|(name, _, _)| !name.is_empty())
            .count() as i64;

        // Per-day details for the heatmap, plus the day set for streaks.
        let mut stmt = conn.prepare(
            "SELECT day, app, dictations, words FROM daily_usage ORDER BY day ASC, app ASC",
        )?;
        let day_rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        struct DayAgg {
            day: String,
            dictations: i64,
            words: i64,
            apps: i64,
            top_app: Option<String>,
            top_count: i64,
        }
        let mut days: std::collections::HashSet<NaiveDate> = std::collections::HashSet::new();
        let mut day_aggs: Vec<DayAgg> = Vec::new();
        for (day, app, dictations, words) in day_rows {
            if let Ok(date) = NaiveDate::parse_from_str(&day, "%Y-%m-%d") {
                days.insert(date);
            }
            if day_aggs.last().map(|agg| agg.day.as_str()) != Some(day.as_str()) {
                day_aggs.push(DayAgg {
                    day,
                    dictations: 0,
                    words: 0,
                    apps: 0,
                    top_app: None,
                    top_count: 0,
                });
            }
            let current = day_aggs.last_mut().expect("just pushed");
            current.dictations += dictations;
            current.words += words;
            if !app.is_empty() {
                current.apps += 1;
                // Rows arrive app-ASC within a day, so on a tie the
                // alphabetically-first app wins (strictly-greater check).
                if dictations > current.top_count {
                    current.top_count = dictations;
                    current.top_app = Some(app);
                }
            }
        }
        let mut daily: Vec<InsightsDay> = day_aggs
            .into_iter()
            .map(|agg| InsightsDay {
                day: agg.day,
                dictations: agg.dictations,
                words: agg.words,
                apps: agg.apps,
                top_app: agg.top_app,
            })
            .collect();

        // Streaks: the aggregate day set carries the post-ship truth, while
        // usage_stats.streak_days still covers a live streak that started
        // before daily_usage existed (its early days were pruned) — take the
        // larger of the two.
        let stored_alive = NaiveDate::parse_from_str(&last_active_day, "%Y-%m-%d")
            .is_ok_and(|last| last == today || Some(last) == today.pred_opt());
        let stored_streak = if stored_alive { streak_days } else { 0 };
        let day_streak = Self::current_streak(&days, today).max(stored_streak);
        let longest_streak = Self::longest_streak(&days).max(day_streak);

        // The heatmap shows ~17 weeks; cap the payload with margin to spare.
        let cutoff = today
            .checked_sub_days(Days::new(140))
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        daily.retain(|entry| entry.day >= cutoff);

        Ok(InsightsStats {
            average_wpm,
            total_words,
            words_this_month,
            month_change_percent,
            cleaned_transcripts: cleaned,
            words_changed,
            app_usage,
            total_apps,
            day_streak,
            longest_streak,
            daily,
        })
    }

    /// Consecutive days with at least one dictation, anchored at today (or
    /// yesterday, so a streak survives until the current day's first
    /// dictation).
    fn current_streak(days: &std::collections::HashSet<NaiveDate>, today: NaiveDate) -> i64 {
        let mut anchor = today;
        if !days.contains(&anchor) {
            match today.pred_opt() {
                Some(yesterday) if days.contains(&yesterday) => anchor = yesterday,
                _ => return 0,
            }
        }
        let mut streak = 0;
        let mut cursor = anchor;
        while days.contains(&cursor) {
            streak += 1;
            match cursor.pred_opt() {
                Some(previous) => cursor = previous,
                None => break,
            }
        }
        streak
    }

    fn longest_streak(days: &std::collections::HashSet<NaiveDate>) -> i64 {
        let mut longest = 0;
        for day in days {
            // Only count from the start of each run.
            if day.pred_opt().is_some_and(|previous| days.contains(&previous)) {
                continue;
            }
            let mut length = 0;
            let mut cursor = *day;
            while days.contains(&cursor) {
                length += 1;
                match cursor.succ_opt() {
                    Some(next) => cursor = next,
                    None => break,
                }
            }
            longest = longest.max(length);
        }
        longest
    }

    pub fn save_entry(
        &self,
        file_name: String,
        transcription_text: String,
        post_process_requested: bool,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let timestamp = Utc::now().timestamp();
        let title = self.format_timestamp_title(timestamp);

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &file_name,
                timestamp,
                false,
                &title,
                &transcription_text,
                &post_processed_text,
                &post_process_prompt,
                post_process_requested,
            ],
        )?;

        let entry = HistoryEntry {
            id: conn.last_insert_rowid(),
            file_name,
            timestamp,
            saved: false,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
        };

        debug!("Saved history entry with id {}", entry.id);
        drop(conn);

        // Publish the new row immediately, then enforce retention on every
        // insert. Previously the configured limit was only applied at startup
        // or when the setting changed, so a running session could grow without
        // bound and the History panel drifted away from the selected count.
        if let Err(e) = (HistoryUpdatePayload::Added {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        match self.cleanup_old_entries() {
            Ok(pruned) if pruned > 0 => {
                // The frontend refetches its first page so rows removed by
                // retention disappear immediately instead of lingering until
                // the History section is reopened.
                if let Err(e) = self.app_handle.emit("history-retention-applied", ()) {
                    error!("Failed to emit retention update: {}", e);
                }
            }
            Ok(_) => {}
            Err(e) => error!("History retention cleanup failed after save: {}", e),
        }

        Ok(entry)
    }

    /// Update an existing history entry with new transcription results (used by retry).
    pub fn update_transcription(
        &self,
        id: i64,
        transcription_text: String,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history
             SET transcription_text = ?1,
                 post_processed_text = ?2,
                 post_process_prompt = ?3
             WHERE id = ?4",
            params![
                transcription_text,
                post_processed_text,
                post_process_prompt,
                id
            ],
        )?;

        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }

        let entry = conn
            .query_row(
                "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                 FROM transcription_history WHERE id = ?1",
                params![id],
                Self::map_history_entry,
            )?;

        debug!("Updated transcription for history entry {}", id);

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Apply the active recording-retention policy and return the number of
    /// database rows removed. Starred rows are never included.
    pub fn cleanup_old_entries(&self) -> Result<usize> {
        let retention_period = crate::settings::get_recording_retention_period(&self.app_handle);

        match retention_period {
            crate::settings::RecordingRetentionPeriod::Never => Ok(0),
            crate::settings::RecordingRetentionPeriod::PreserveLimit => {
                let limit = crate::settings::get_history_limit(&self.app_handle);
                self.cleanup_by_count(limit)
            }
            _ => self.cleanup_by_time(retention_period),
        }
    }

    fn delete_entries_and_files(&self, entries: &[(i64, String)]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;
        let mut deleted_count = 0;

        for (id, file_name) in entries {
            let rows_deleted = conn.execute(
                "DELETE FROM transcription_history WHERE id = ?1",
                params![id],
            )?;
            if rows_deleted == 0 {
                continue;
            }
            deleted_count += rows_deleted;

            // Older builds named WAVs with second-level timestamps, so two
            // rows could reference the same file. Remove it only after the last
            // referencing row is gone; otherwise pruning one row breaks audio
            // playback for another row that is still visible.
            let remaining_references: i64 = conn.query_row(
                "SELECT COUNT(*) FROM transcription_history WHERE file_name = ?1",
                params![file_name],
                |row| row.get(0),
            )?;
            // Transform executions have no recording (empty file_name); an
            // empty name would join() to the recordings dir itself.
            if remaining_references == 0 && !file_name.is_empty() {
                let file_path = self.recordings_dir.join(file_name);
                if file_path.exists() {
                    if let Err(e) = fs::remove_file(&file_path) {
                        error!("Failed to delete WAV file {}: {}", file_name, e);
                    } else {
                        debug!("Deleted old WAV file: {}", file_name);
                    }
                }
            }
        }

        Ok(deleted_count)
    }

    fn entries_beyond_unsaved_limit(conn: &Connection, limit: usize) -> Result<Vec<(i64, String)>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_name
             FROM transcription_history
             WHERE saved = 0
             ORDER BY timestamp DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;
        let entries = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries.into_iter().skip(limit).collect())
    }

    fn cleanup_by_count(&self, limit: usize) -> Result<usize> {
        let conn = self.get_connection()?;
        let entries_to_delete = Self::entries_beyond_unsaved_limit(&conn, limit)?;
        let deleted_count = self.delete_entries_and_files(&entries_to_delete)?;

        if deleted_count > 0 {
            debug!("Cleaned up {} old history entries by count", deleted_count);
        }

        Ok(deleted_count)
    }

    fn unsaved_entries_before(
        conn: &Connection,
        cutoff_timestamp: i64,
    ) -> Result<Vec<(i64, String)>> {
        let mut stmt = conn.prepare(
            "SELECT id, file_name
             FROM transcription_history
             WHERE saved = 0 AND timestamp < ?1",
        )?;
        let rows = stmt.query_map(params![cutoff_timestamp], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn cleanup_by_time(
        &self,
        retention_period: crate::settings::RecordingRetentionPeriod,
    ) -> Result<usize> {
        let conn = self.get_connection()?;

        // Calculate cutoff timestamp (current time minus retention period)
        let now = Utc::now().timestamp();
        let cutoff_timestamp = match retention_period {
            crate::settings::RecordingRetentionPeriod::Days3 => now - (3 * 24 * 60 * 60), // 3 days in seconds
            crate::settings::RecordingRetentionPeriod::Weeks2 => now - (2 * 7 * 24 * 60 * 60), // 2 weeks in seconds
            crate::settings::RecordingRetentionPeriod::Months3 => now - (3 * 30 * 24 * 60 * 60), // 3 months in seconds (approximate)
            _ => unreachable!("Should not reach here"),
        };

        let entries_to_delete = Self::unsaved_entries_before(&conn, cutoff_timestamp)?;
        let deleted_count = self.delete_entries_and_files(&entries_to_delete)?;

        if deleted_count > 0 {
            debug!(
                "Cleaned up {} old history entries based on retention period",
                deleted_count
            );
        }

        Ok(deleted_count)
    }

    pub async fn get_history_entries(
        &self,
        cursor: Option<i64>,
        limit: Option<usize>,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let limit = limit.map(|l| l.min(100));

        let mut entries: Vec<HistoryEntry> = match (cursor, limit) {
            (Some(cursor_id), Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     WHERE id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;
                let result = stmt
                    .query_map(params![cursor_id, fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (None, Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let result = stmt
                    .query_map(params![fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (_, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     ORDER BY id DESC",
                )?;
                let result = stmt
                    .query_map([], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
        };

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    #[cfg(test)]
    fn get_latest_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
             FROM transcription_history
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    /// Get the latest entry with non-empty transcription text.
    pub fn get_latest_completed_entry(&self) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        Self::get_latest_completed_entry_with_conn(&conn)
    }

    fn get_latest_completed_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
             FROM transcription_history
             WHERE transcription_text != ''
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    pub async fn toggle_saved_status(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        // Get current saved status
        let current_saved: bool = conn.query_row(
            "SELECT saved FROM transcription_history WHERE id = ?1",
            params![id],
            |row| row.get("saved"),
        )?;

        let new_saved = !current_saved;

        conn.execute(
            "UPDATE transcription_history SET saved = ?1 WHERE id = ?2",
            params![new_saved, id],
        )?;

        debug!("Toggled saved status for entry {}: {}", id, new_saved);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Toggled { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    pub fn get_audio_file_path(&self, file_name: &str) -> PathBuf {
        self.recordings_dir.join(file_name)
    }

    pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
             FROM transcription_history
             WHERE id = ?1",
        )?;

        let entry = stmt.query_row([id], Self::map_history_entry).optional()?;

        Ok(entry)
    }

    pub async fn delete_entry(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        let file_name = conn
            .query_row(
                "SELECT file_name FROM transcription_history WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        conn.execute(
            "DELETE FROM transcription_history WHERE id = ?1",
            params![id],
        )?;

        // Old rows may share a second-resolution filename. Keep the WAV while
        // any other row still references it.
        if let Some(file_name) = file_name {
            let remaining_references: i64 = conn.query_row(
                "SELECT COUNT(*) FROM transcription_history WHERE file_name = ?1",
                params![&file_name],
                |row| row.get(0),
            )?;
            // See the retention guard above: empty file_name = no recording.
            if remaining_references == 0 && !file_name.is_empty() {
                let file_path = self.get_audio_file_path(&file_name);
                if file_path.exists() {
                    if let Err(e) = fs::remove_file(&file_path) {
                        error!("Failed to delete audio file {}: {}", file_name, e);
                    }
                }
            }
        }

        debug!("Deleted history entry with id: {}", id);

        if let Err(e) = (HistoryUpdatePayload::Deleted { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    fn format_timestamp_title(&self, timestamp: i64) -> String {
        if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
            // Convert UTC to local timezone
            let local_datetime = utc_datetime.with_timezone(&Local);
            local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
        } else {
            format!("Recording {}", timestamp)
        }
    }

    // -----------------------------------------------------------------------
    // Assistant conversation history
    // -----------------------------------------------------------------------

    /// Keep at most this many assistant conversations so the table can't grow
    /// without bound. Generous on purpose — conversations are small JSON rows
    /// and users expect their chat history to stick around.
    const ASSISTANT_SESSION_CAP: i64 = 500;

    fn map_assistant_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<AssistantHistoryEntry> {
        let messages_json: String = row.get("messages")?;
        // A malformed row shouldn't take down the whole list — fall back to an
        // empty transcript rather than erroring the query.
        let messages = serde_json::from_str::<Vec<ChatMessage>>(&messages_json).unwrap_or_default();
        Ok(AssistantHistoryEntry {
            id: row.get("id")?,
            timestamp: row.get("timestamp")?,
            updated_at: row.get("updated_at")?,
            title: row.get("title")?,
            messages,
        })
    }

    /// Derive a short, human-readable title from the first user message.
    fn derive_assistant_title(messages: &[ChatMessage]) -> String {
        let raw = messages
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        // Stored user messages may carry the screenshot marker; drop it.
        let cleaned = raw.replace(crate::assistant::SCREENSHOT_MARKER, "");
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            return "Conversation".to_string();
        }
        let title: String = trimmed.chars().take(80).collect();
        if trimmed.chars().count() > 80 {
            format!("{}…", title)
        } else {
            title
        }
    }

    /// Insert a new assistant conversation row and return it.
    pub fn create_assistant_session(
        &self,
        messages: &[ChatMessage],
    ) -> Result<AssistantHistoryEntry> {
        let now = Utc::now().timestamp();
        let title = Self::derive_assistant_title(messages);
        let messages_json = serde_json::to_string(messages)?;

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO assistant_history (timestamp, updated_at, title, messages)
             VALUES (?1, ?2, ?3, ?4)",
            params![now, now, &title, &messages_json],
        )?;
        let id = conn.last_insert_rowid();

        self.cleanup_assistant_sessions()?;

        debug!("Saved assistant session with id {}", id);

        Ok(AssistantHistoryEntry {
            id,
            timestamp: now,
            updated_at: now,
            title,
            messages: messages.to_vec(),
        })
    }

    /// Update an existing assistant conversation in place. Returns `Ok(None)`
    /// when the row no longer exists (e.g. it was deleted from the History
    /// view), so the caller can decide to create a fresh session instead.
    pub fn update_assistant_session(
        &self,
        id: i64,
        messages: &[ChatMessage],
    ) -> Result<Option<AssistantHistoryEntry>> {
        let now = Utc::now().timestamp();
        let title = Self::derive_assistant_title(messages);
        let messages_json = serde_json::to_string(messages)?;

        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE assistant_history
             SET updated_at = ?1, title = ?2, messages = ?3
             WHERE id = ?4",
            params![now, &title, &messages_json, id],
        )?;

        if updated == 0 {
            return Ok(None);
        }

        let timestamp: i64 = conn.query_row(
            "SELECT timestamp FROM assistant_history WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        Ok(Some(AssistantHistoryEntry {
            id,
            timestamp,
            updated_at: now,
            title,
            messages: messages.to_vec(),
        }))
    }

    /// Page through assistant conversations, newest first (keyset pagination
    /// on `id`, mirroring `get_history_entries`).
    pub async fn get_assistant_history_entries(
        &self,
        cursor: Option<i64>,
        limit: Option<usize>,
    ) -> Result<PaginatedAssistantHistory> {
        let conn = self.get_connection()?;
        let limit = limit.map(|l| l.min(200));

        let mut entries: Vec<AssistantHistoryEntry> = match (cursor, limit) {
            (Some(cursor_id), Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, timestamp, updated_at, title, messages
                     FROM assistant_history
                     WHERE id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;
                let result = stmt
                    .query_map(params![cursor_id, fetch_count], Self::map_assistant_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (None, Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, timestamp, updated_at, title, messages
                     FROM assistant_history
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let result = stmt
                    .query_map(params![fetch_count], Self::map_assistant_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (_, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, timestamp, updated_at, title, messages
                     FROM assistant_history
                     ORDER BY id DESC",
                )?;
                let result = stmt
                    .query_map([], Self::map_assistant_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
        };

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedAssistantHistory { entries, has_more })
    }

    pub fn delete_assistant_session(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM assistant_history WHERE id = ?1", params![id])?;
        debug!("Deleted assistant session with id: {}", id);
        Ok(())
    }

    /// Fetch a single assistant conversation by id (for resuming it in the
    /// panel from the History view). `Ok(None)` when the row no longer exists.
    pub fn get_assistant_session(&self, id: i64) -> Result<Option<AssistantHistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, updated_at, title, messages
             FROM assistant_history
             WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], Self::map_assistant_entry)?;
        match rows.next() {
            Some(entry) => Ok(Some(entry?)),
            None => Ok(None),
        }
    }

    /// Trim the oldest conversations beyond [`Self::ASSISTANT_SESSION_CAP`].
    fn cleanup_assistant_sessions(&self) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "DELETE FROM assistant_history
             WHERE id NOT IN (
                 SELECT id FROM assistant_history ORDER BY id DESC LIMIT ?1
             )",
            params![Self::ASSISTANT_SESSION_CAP],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                saved BOOLEAN NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                transcription_text TEXT NOT NULL,
                post_processed_text TEXT,
                post_process_prompt TEXT,
                post_process_requested BOOLEAN NOT NULL DEFAULT 0
            );",
        )
        .expect("create transcription_history table");
        conn
    }

    fn insert_entry(conn: &Connection, timestamp: i64, text: &str, post_processed: Option<&str>) {
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,

                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("speakoflow-{}.wav", timestamp),
                timestamp,
                false,
                format!("Recording {}", timestamp),
                text,
                post_processed,
                Option::<String>::None,
                false,
            ],
        )
        .expect("insert history entry");
    }

    #[test]
    fn legacy_history_schema_migrates_to_current_baseline() {
        let mut conn = Connection::open_in_memory().expect("open legacy database");
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                saved BOOLEAN NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                transcription_text TEXT NOT NULL
            );
            INSERT INTO transcription_history (
                file_name, timestamp, saved, title, transcription_text
            ) VALUES ('legacy.wav', 10, 0, 'Legacy', 'hello');
            PRAGMA user_version = 1;",
        )
        .expect("create legacy schema");

        Migrations::new(MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .expect("migrate legacy schema");

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(transcription_history)")
            .expect("prepare table info")
            .query_map([], |row| row.get(1))
            .expect("query table info")
            .collect::<rusqlite::Result<_>>()
            .expect("collect columns");
        for required in [
            "post_processed_text",
            "post_process_prompt",
            "post_process_requested",
        ] {
            assert!(columns.iter().any(|column| column == required));
        }

        let requested: bool = conn
            .query_row(
                "SELECT post_process_requested FROM transcription_history WHERE file_name = 'legacy.wav'",
                [],
                |row| row.get(0),
            )
            .expect("read migrated row");
        assert!(!requested);
    }

    #[test]
    fn get_latest_entry_returns_none_when_empty() {
        let conn = setup_conn();
        let entry = HistoryManager::get_latest_entry_with_conn(&conn).expect("fetch latest entry");
        assert!(entry.is_none());
    }

    #[test]
    fn get_latest_entry_returns_newest_entry() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "first", None);
        insert_entry(&conn, 200, "second", Some("processed"));

        let entry = HistoryManager::get_latest_entry_with_conn(&conn)
            .expect("fetch latest entry")
            .expect("entry exists");

        assert_eq!(entry.timestamp, 200);
        assert_eq!(entry.transcription_text, "second");
        assert_eq!(entry.post_processed_text.as_deref(), Some("processed"));
    }

    #[test]
    fn get_latest_completed_entry_skips_empty_entries() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "completed", None);
        insert_entry(&conn, 200, "", None);

        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest completed entry")
            .expect("completed entry exists");

        assert_eq!(entry.timestamp, 100);
        assert_eq!(entry.transcription_text, "completed");
    }

    #[test]
    fn count_retention_keeps_starred_recordings() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "oldest", None);
        insert_entry(&conn, 200, "middle", None);
        insert_entry(&conn, 300, "newest", None);
        insert_entry(&conn, 50, "starred oldest", None);
        conn.execute(
            "UPDATE transcription_history SET saved = 1 WHERE timestamp = 50",
            [],
        )
        .expect("star recording");

        let selected = HistoryManager::entries_beyond_unsaved_limit(&conn, 1)
            .expect("select count-retention entries");
        let selected_files: Vec<&str> = selected.iter().map(|(_, name)| name.as_str()).collect();

        assert_eq!(
            selected_files,
            vec!["speakoflow-200.wav", "speakoflow-100.wav"]
        );
        assert!(!selected_files.contains(&"speakoflow-50.wav"));
    }

    #[test]
    fn time_retention_keeps_starred_recordings() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "old unsaved", None);
        insert_entry(&conn, 50, "old starred", None);
        insert_entry(&conn, 300, "new unsaved", None);
        conn.execute(
            "UPDATE transcription_history SET saved = 1 WHERE timestamp = 50",
            [],
        )
        .expect("star recording");

        let selected = HistoryManager::unsaved_entries_before(&conn, 200)
            .expect("select time-retention entries");
        let selected_files: Vec<&str> = selected.iter().map(|(_, name)| name.as_str()).collect();

        assert_eq!(selected_files, vec!["speakoflow-100.wav"]);
        assert!(!selected_files.contains(&"speakoflow-50.wav"));
    }

    // -------------------------------------------------------------------
    // Insights
    // -------------------------------------------------------------------

    /// Fully migrated in-memory database (usage_stats + daily_usage present).
    fn setup_migrated_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        Migrations::new(MIGRATIONS.to_vec())
            .to_latest(&mut conn)
            .expect("run migrations");
        conn
    }

    fn insert_daily(conn: &Connection, day: &str, app: &str, dictations: i64, words: i64) {
        conn.execute(
            "INSERT INTO daily_usage (day, app, dictations, words) VALUES (?1, ?2, ?3, ?4)",
            params![day, app, dictations, words],
        )
        .expect("insert daily_usage row");
    }

    #[test]
    fn changed_word_count_is_case_insensitive_multiset_difference() {
        // "go." differs from "go" (punctuation stays attached to the token).
        assert_eq!(changed_word_count("i want to go", "I want to go."), 1);
        // Identical up to case and order: nothing changed.
        assert_eq!(changed_word_count("World hello", "hello world"), 0);
        // A word used more often in the final text counts the extra uses.
        assert_eq!(changed_word_count("very good", "very very good"), 1);
        assert_eq!(changed_word_count("", "brand new words"), 3);
    }

    #[test]
    fn insights_on_empty_database_are_all_zero() {
        let conn = setup_migrated_conn();
        let today = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        let stats = HistoryManager::insights_with_conn(&conn, today).expect("compute insights");

        assert_eq!(stats.average_wpm, 0);
        assert_eq!(stats.total_words, 0);
        assert_eq!(stats.words_this_month, 0);
        assert_eq!(stats.month_change_percent, None);
        assert_eq!(stats.cleaned_transcripts, 0);
        assert!(stats.app_usage.is_empty());
        assert_eq!(stats.day_streak, 0);
        assert_eq!(stats.longest_streak, 0);
        assert!(stats.daily.is_empty());
    }

    #[test]
    fn insights_aggregate_apps_months_and_streaks() {
        let conn = setup_migrated_conn();
        let today = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();

        conn.execute(
            "INSERT INTO usage_stats (id, total_words, total_speech_ms, streak_days,
                                      last_active_day, cleaned_transcripts, words_changed)
             VALUES (1, 1000, 600000, 2, '2026-08-16', 7, 42)",
            [],
        )
        .expect("seed usage_stats");

        insert_daily(&conn, "2026-07-10", "Slack", 4, 200);
        insert_daily(&conn, "2026-08-15", "Code", 6, 300);
        insert_daily(&conn, "2026-08-15", "Slack", 2, 60);
        insert_daily(&conn, "2026-08-16", "", 2, 100);
        // Tie on dictations for the 16th: alphabetically-first app wins.
        insert_daily(&conn, "2026-08-16", "Chrome", 3, 90);
        insert_daily(&conn, "2026-08-16", "Slack", 3, 50);

        let stats = HistoryManager::insights_with_conn(&conn, today).expect("compute insights");

        // 1000 words over 10 minutes of speech.
        assert_eq!(stats.average_wpm, 100);
        assert_eq!(stats.total_words, 1000);
        assert_eq!(stats.words_this_month, 600);
        // (600 - 200) / 200 = +200%.
        assert_eq!(stats.month_change_percent, Some(200));
        assert_eq!(stats.cleaned_transcripts, 7);
        assert_eq!(stats.words_changed, 42);

        // Sorted by count desc, then name asc; '' groups as its own row.
        let names: Vec<(&str, i64, i64, i64)> = stats
            .app_usage
            .iter()
            .map(|usage| (usage.name.as_str(), usage.count, usage.percent, usage.words))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Slack", 9, 45, 310),
                ("Code", 6, 30, 300),
                ("Chrome", 3, 15, 90),
                ("", 2, 10, 100),
            ]
        );
        assert_eq!(stats.total_apps, 3);

        // 15th + 16th active: 2-day current streak; longest is also 2.
        assert_eq!(stats.day_streak, 2);
        assert_eq!(stats.longest_streak, 2);

        let day16 = stats
            .daily
            .iter()
            .find(|day| day.day == "2026-08-16")
            .expect("day present");
        assert_eq!(day16.dictations, 8);
        assert_eq!(day16.words, 240);
        assert_eq!(day16.apps, 2);
        assert_eq!(day16.top_app.as_deref(), Some("Chrome"));
    }

    #[test]
    fn insights_streak_survives_until_first_dictation_of_the_day() {
        let conn = setup_migrated_conn();
        insert_daily(&conn, "2026-08-14", "Code", 1, 10);
        insert_daily(&conn, "2026-08-15", "Code", 1, 10);

        let today = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        let stats = HistoryManager::insights_with_conn(&conn, today).expect("compute insights");
        assert_eq!(stats.day_streak, 2);

        // Two days without dictating: the streak is over.
        let later = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        let stats = HistoryManager::insights_with_conn(&conn, later).expect("compute insights");
        assert_eq!(stats.day_streak, 0);
        assert_eq!(stats.longest_streak, 2);
    }

    #[test]
    fn insights_prefer_stored_streak_when_daily_rows_were_never_written() {
        // A live streak that started before daily_usage existed: usage_stats
        // says 5 days, the aggregate table only knows about today.
        let conn = setup_migrated_conn();
        let today = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        conn.execute(
            "INSERT INTO usage_stats (id, streak_days, last_active_day)
             VALUES (1, 5, '2026-08-16')",
            [],
        )
        .expect("seed usage_stats");
        insert_daily(&conn, "2026-08-16", "Code", 1, 10);

        let stats = HistoryManager::insights_with_conn(&conn, today).expect("compute insights");
        assert_eq!(stats.day_streak, 5);
        assert_eq!(stats.longest_streak, 5);
    }

    #[test]
    fn backfill_seeds_daily_usage_once_and_skips_transform_rows() {
        let conn = setup_migrated_conn();
        let timestamp = 1_755_000_000; // fixed instant; expected day computed below
        insert_entry(&conn, timestamp, "one two three", None);
        insert_entry(&conn, timestamp + 60, "raw words here", Some("cleaned words"));
        // Transform executions have no recording and are not dictations.
        conn.execute(
            "INSERT INTO transcription_history (
                file_name, timestamp, saved, title, transcription_text, post_process_requested
            ) VALUES ('', ?1, 0, 'Transform', 'transformed text', 0)",
            params![timestamp + 120],
        )
        .expect("insert transform row");

        HistoryManager::backfill_daily_usage(&conn).expect("backfill");

        let expected_day = DateTime::from_timestamp(timestamp, 0)
            .unwrap()
            .with_timezone(&Local)
            .format("%Y-%m-%d")
            .to_string();
        let (dictations, words): (i64, i64) = conn
            .query_row(
                "SELECT dictations, words FROM daily_usage WHERE day = ?1 AND app = ''",
                params![expected_day],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("backfilled row");
        // 3 raw words + 2 cleaned words (cleaned text wins when present).
        assert_eq!(dictations, 2);
        assert_eq!(words, 5);

        // Idempotent: a second run must not double the numbers.
        HistoryManager::backfill_daily_usage(&conn).expect("backfill again");
        let dictations_after: i64 = conn
            .query_row(
                "SELECT SUM(dictations) FROM daily_usage",
                [],
                |row| row.get(0),
            )
            .expect("sum");
        assert_eq!(dictations_after, 2);
    }
}
