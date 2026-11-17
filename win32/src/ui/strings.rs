//! Every user-visible string lives here, so a translation later does not mean
//! hunting through the code. v1 is English only.
#![allow(dead_code)] // strings consumed progressively across F2/F3 screens

pub const APP_NAME: &str = "Focus";
pub const CLOSE_LABEL: &str = "Close";
pub const CANCEL_LABEL: &str = "Cancel";
pub const CONFIRM_LABEL: &str = "Confirm";
pub const SELECT_PLACEHOLDER: &str = "Select…";
pub const SELECT_MENU_LABEL: &str = "Options";
pub const SORT_ASC_SUFFIX: &str = "sorted ascending";
pub const SORT_DESC_SUFFIX: &str = "sorted descending";
pub const LIST_EMPTY: &str = "Nothing here yet";
pub const SCROLL_BAR_LABEL: &str = "Scroll list";

// ── Sidebar & navigation ─────────────────────────────────────────────────

pub const NAV_CALENDAR: &str = "Calendar";
pub const NAV_ACTIVITY: &str = "Activity";
pub const NAV_REPORTS: &str = "Reports";
pub const NAV_PROJECTS: &str = "Projects";
pub const NAV_SETTINGS: &str = "Settings";
pub const SIDEBAR_THIS_WEEK: &str = "This week";
pub const SIDEBAR_DEMO_BADGE: &str = "Demo data";
pub const SIDEBAR_TIMER_LIVE: &str = "Timer running";
pub const SIDEBAR_NAV_LABEL: &str = "Navigation";
pub const SIDEBAR_CREATE_PROJECT: &str = "Create a project";

// ── Period navigation ────────────────────────────────────────────────────

pub const PERIOD_GROUP_LABEL: &str = "Period";
pub const PERIOD_PREV_LABEL: &str = "Previous period";
pub const PERIOD_NEXT_LABEL: &str = "Next period";
pub const PERIOD_RESET_HINT: &str = "Back to now";
pub const PERIOD_LENGTH_LABEL: &str = "Period length";
pub const PERIOD_DAY: &str = "Day";
pub const PERIOD_WEEK: &str = "Week";
pub const PERIOD_MONTH: &str = "Month";
pub const PERIOD_YEAR: &str = "Year";
pub const PERIOD_ALL: &str = "All time";
pub const PERIOD_TODAY: &str = "Today";
pub const PERIOD_THIS_WEEK_PREFIX: &str = "This week";

// ── Welcome screen ───────────────────────────────────────────────────────

pub const WELCOME_HEADLINE: &str = "See where your time actually goes";
pub const WELCOME_LEAD: &str =
    "Log time against your projects, and let Focus quietly keep track of what you worked on, so nothing you did slips through. Change any source later in Settings.";
pub const WELCOME_SRC_APPS_TITLE: &str = "Apps and windows";
pub const WELCOME_SRC_APPS_HINT: &str =
    "The foreground app and its window title, every 5 seconds. Pauses when idle, locked, or asleep.";
pub const WELCOME_SRC_BROWSER_TITLE: &str = "Browser tabs";
pub const WELCOME_SRC_BROWSER_HINT: &str =
    "The active tab's title in Chrome, Edge, Brave, Firefox, and Arc via Windows UI Automation.";
pub const WELCOME_SRC_CLAUDE_TITLE: &str = "Claude Code sessions";
pub const WELCOME_SRC_CLAUDE_HINT: &str =
    "Which project and session you worked in from ~/.claude/projects, and when. Never the conversation.";
pub const WELCOME_PRIVACY: &str =
    "Everything stays in one SQLite file on this PC. No account, no cloud, no analytics.";
pub const WELCOME_START_BTN: &str = "Start using Focus";

// ── Settings screen ──────────────────────────────────────────────────────

pub const SETTINGS_TITLE: &str = "Settings";
pub const SETTINGS_SEC_MONITOR: &str = "Activity monitor";
pub const SETTINGS_SEC_PRIVACY: &str = "Privacy exclusions";
pub const SETTINGS_SEC_TRACKING: &str = "Time tracking & calendar";
pub const SETTINGS_SEC_DATA: &str = "Data & exports";

pub const SETTINGS_RECORD_LABEL: &str = "Record activity";
pub const SETTINGS_RECORD_HINT: &str =
    "Samples the foreground app and window every 5 seconds into the local database.";
pub const SETTINGS_BROWSER_LABEL: &str = "Browser tabs";
pub const SETTINGS_BROWSER_HINT: &str =
    "Reads the active tab's title in supported browsers. Requires Record activity.";
pub const SETTINGS_CLAUDE_LABEL: &str = "Claude Code sessions";
pub const SETTINGS_CLAUDE_HINT: &str =
    "Reads session logs in ~/.claude/projects: project, session title, and timestamps only.";
pub const SETTINGS_BUSY_LABEL: &str = "Keep active during calls";
pub const SETTINGS_BUSY_HINT: &str =
    "Prevent idle pause while microphone, camera, or full-screen presentation is active.";
pub const SETTINGS_IDLE_LABEL: &str = "Pause when idle";
pub const SETTINGS_IDLE_HINT: &str =
    "Stop counting activity after this long without keyboard or mouse input.";

pub const SETTINGS_EXCL_APPS_LABEL: &str = "Excluded apps";
pub const SETTINGS_EXCL_APPS_HINT: &str =
    "Comma-separated app names ignored by the recorder (password managers default-excluded).";
pub const SETTINGS_EXCL_APPS_PLACEHOLDER: &str = "1Password, Bitwarden, KeePass";
pub const SETTINGS_EXCL_TITLES_LABEL: &str = "Excluded window keywords";
pub const SETTINGS_EXCL_TITLES_HINT: &str =
    "Comma-separated keywords; windows whose title matches are stored without title text.";
pub const SETTINGS_EXCL_TITLES_PLACEHOLDER: &str = "Private, Incognito, Secret";

pub const SETTINGS_CURRENCY_LABEL: &str = "Default currency";
pub const SETTINGS_CURRENCY_HINT: &str = "For new projects. Each project keeps its own currency.";
pub const SETTINGS_DAY_START_LABEL: &str = "Week view starts at";
pub const SETTINGS_DAY_START_HINT: &str = "The hour the weekly calendar scrolls to when it opens.";
pub const SETTINGS_FIRST_DAY_LABEL: &str = "First day of week";
pub const SETTINGS_FIRST_DAY_HINT: &str =
    "Controls the first column of the Calendar and Weekly views.";
pub const SETTINGS_TIME_FORMAT_LABEL: &str = "Time format";
pub const SETTINGS_TIME_FORMAT_HINT: &str =
    "Choose system default, 24-hour clocks, or 12-hour AM/PM clocks.";

pub const SETTINGS_FIRST_DAY_SYSTEM: &str = "System default";
pub const SETTINGS_FIRST_DAY_MONDAY: &str = "Monday";
pub const SETTINGS_FIRST_DAY_SUNDAY: &str = "Sunday";
pub const SETTINGS_TIME_SYSTEM: &str = "System default";
pub const SETTINGS_TIME_24H: &str = "24-hour (14:30)";
pub const SETTINGS_TIME_12H: &str = "12-hour (2:30 PM)";
pub const SETTINGS_TIME_AM: &str = "AM";
pub const SETTINGS_TIME_PM: &str = "PM";
pub const SETTINGS_IDLE_UNIT: &str = "min";

pub const SETTINGS_CSV_LABEL: &str = "Standard CSV by default";
pub const SETTINGS_CSV_HINT: &str =
    "Off writes semicolon/decimal-comma UTF-8 BOM CSV for Excel; on writes comma/dot RFC CSV.";
pub const SETTINGS_DELETE_LABEL: &str = "Delete activity history";
pub const SETTINGS_DELETE_HINT: &str =
    "Removes recorded activity. Time entries and projects are kept.";
pub const SETTINGS_DELETE_CONFIRM_HINT: &str =
    "This removes every recorded app, window, and Claude Code session. Time entries and projects stay. It cannot be undone.";
pub const SETTINGS_DELETE_BTN: &str = "Delete…";
pub const SETTINGS_DELETE_KEEP_BTN: &str = "Keep";
pub const SETTINGS_DELETE_CONFIRM_BTN: &str = "Delete activity history";

// ── Projects & Project Dialog ────────────────────────────────────────────

pub const PROJECTS_TITLE: &str = "Projects";
pub const PROJECTS_NEW_BTN: &str = "New project";
pub const PROJECTS_SHOW_ARCHIVED: &str = "Show archived";
pub const PROJECTS_HIDE_ARCHIVED: &str = "Hide archived";
pub const PROJECTS_EMPTY_TITLE: &str = "No projects yet";
pub const PROJECTS_EMPTY_SUB: &str =
    "Create your first project to organize time entries, hourly rates, and reports.";
pub const PROJECTS_COL_NAME: &str = "Project";
pub const PROJECTS_COL_CLIENT: &str = "Client";
pub const PROJECTS_COL_RATE: &str = "Rate";
pub const PROJECTS_COL_MONTH: &str = "This month";
pub const PROJECTS_COL_TOTAL: &str = "Total";
pub const PROJECTS_COL_EARNED: &str = "Earned";
pub const PROJECTS_COL_STATUS: &str = "Status";
pub const PROJECTS_STATUS_ACTIVE: &str = "Active";
pub const PROJECTS_STATUS_ARCHIVED: &str = "Archived";
pub const PROJECTS_INTERNAL: &str = "Internal";
pub const PROJECTS_COUNT_ONE: &str = "1 project";
pub const PROJECTS_COUNT_MANY_SUFFIX: &str = "projects";
pub const PROJECTS_ARCHIVED_SHOW_PREFIX: &str = "Show";
pub const PROJECTS_ARCHIVED_SUFFIX: &str = "archived";
pub const PROJECTS_NON_BILLABLE: &str = "Non-billable";
pub const PROJECTS_NO_PROJECT: &str = "No project";

pub const PROJECT_DLG_NEW: &str = "New project";
pub const PROJECT_DLG_EDIT: &str = "Edit project";
pub const PROJECT_FIELD_NAME: &str = "Project name";
pub const PROJECT_FIELD_NAME_HINT: &str = "Website redesign";
pub const PROJECT_FIELD_CLIENT: &str = "Client";
pub const PROJECT_FIELD_CLIENT_HINT: &str = "Client (optional)";
pub const PROJECT_FIELD_RATE: &str = "Hourly rate";
pub const PROJECT_FIELD_RATE_HINT: &str = "0 = internal";
pub const PROJECT_FIELD_CURRENCY: &str = "Currency";
pub const PROJECT_FIELD_COLOR: &str = "Color";
pub const PROJECT_FIELD_BILLABLE: &str = "Billable by default";
pub const PROJECT_ERR_NO_NAME: &str = "Give the project a name.";
pub const PROJECT_ERR_BAD_RATE: &str = "Hourly rate must be a valid non-negative number.";
pub const PROJECT_BTN_SAVE: &str = "Save";
pub const PROJECT_BTN_CREATE: &str = "Create project";
pub const PROJECT_BTN_ARCHIVE: &str = "Archive";
pub const PROJECT_BTN_UNARCHIVE: &str = "Restore";
pub const PROJECT_BTN_DELETE: &str = "Delete";
pub const PROJECT_BTN_CONFIRM_DELETE: &str = "Confirm delete";
pub const PROJECT_BTN_VIEW_REPORT: &str = "View report";

// ── Timer Bar & Entry Editor ─────────────────────────────────────────────

pub const TIMER_PLACEHOLDER: &str = "What are you working on?";
pub const TIMER_NO_PROJECT: &str = "No project";
pub const TIMER_BILLABLE: &str = "Billable";
pub const TIMER_START: &str = "Start timer";
pub const TIMER_STOP: &str = "Stop timer";

pub const ENTRY_DLG_NEW: &str = "New time entry";
pub const ENTRY_DLG_EDIT: &str = "Edit time entry";
pub const ENTRY_DLG_RUNNING: &str = "Running timer";
pub const ENTRY_FIELD_WHAT: &str = "Description";
pub const ENTRY_FIELD_PROJECT: &str = "Project";
pub const ENTRY_FIELD_DATE: &str = "Date";
pub const ENTRY_FIELD_START: &str = "Start";
pub const ENTRY_FIELD_END: &str = "End";
pub const ENTRY_FIELD_BILLABLE: &str = "Billable";
pub const ENTRY_ERROR_TIME: &str = "Enter times like 09:30 or 930";
pub const ENTRY_ERROR_SPAN: &str = "Duration cannot exceed 24 hours";
pub const ENTRY_BTN_STOP_NOW: &str = "Stop now";

// ── Calendar ─────────────────────────────────────────────────────────────

pub const CAL_TITLE: &str = "Calendar";
pub const CAL_MODE_MIX: &str = "Mix";
pub const CAL_MODE_TIME: &str = "Time";
pub const CAL_MODE_ACTIVITY: &str = "Activity";
pub const CAL_PREV_WEEK: &str = "Previous week";
pub const CAL_NEXT_WEEK: &str = "Next week";
pub const CAL_THIS_WEEK: &str = "This week";
pub const CAL_THIS_WEEK_PREFIX: &str = "This week";
pub const CAL_DAY_TODAY: &str = "Today";
pub const CAL_GRID_LABEL: &str = "Weekly time grid";
pub const CAL_POPOVER_LOG_BTN: &str = "Log as time entry";
pub const CAL_POPOVER_CLAUDE_SECTION: &str = "Claude Code";
pub const CAL_POPOVER_APPS_SECTION: &str = "Apps & windows";

// ── Activity Screen ──────────────────────────────────────────────────────

pub const ACT_TITLE: &str = "Activity";
pub const ACT_STAT_ACTIVE: &str = "Active time";
pub const ACT_STAT_APPS: &str = "Apps";
pub const ACT_STAT_BROWSER: &str = "Browser";
pub const ACT_STAT_CLAUDE: &str = "Claude Code";
pub const ACT_HEATMAP_TITLE: &str = "When you were at it";
pub const ACT_DAY_HEATMAP_TITLE: &str = "Quarter-hour density";
pub const ACT_DAILY_BARS_TITLE: &str = "Apps over time";
pub const ACT_APPS_TITLE: &str = "Apps";
pub const ACT_WHERE_TITLE: &str = "Where the time went";
pub const ACT_CLAUDE_MODE_PROJECTS: &str = "Projects";
pub const ACT_CLAUDE_MODE_SESSIONS: &str = "Sessions";
pub const ACT_CLEAR_FILTER: &str = "Clear filter";
pub const ACT_EMPTY: &str = "No activity recorded in this period";
pub const ACT_EMPTY_DISABLED: &str = "Activity recording is turned off";

// ── Reports & Project Report ─────────────────────────────────────────────

pub const REP_TITLE: &str = "Reports";
pub const REP_ALL_PROJECTS: &str = "All projects";
pub const REP_NO_PROJECT: &str = "No project";
pub const REP_EXPORT_CSV: &str = "Export CSV";
pub const REP_EXPORT_DLG_TITLE: &str = "Export time entries";
pub const REP_EXPORT_EXCEL: &str = "Excel (locale separator, UTF-8 BOM)";
pub const REP_EXPORT_STANDARD: &str = "Standard CSV (comma & dot)";
pub const REP_STAT_TOTAL: &str = "Total time";
pub const REP_STAT_BILLABLE: &str = "Billable";
pub const REP_STAT_EARNINGS: &str = "Earnings";
pub const REP_STAT_ENTRIES: &str = "Entries";
pub const REP_CHART_HOURS: &str = "Hours";
pub const REP_CHART_EARNINGS: &str = "Earnings";
pub const REP_BY_PROJECT: &str = "By project";
pub const REP_ENTRIES_TITLE: &str = "Entries";
pub const REP_EMPTY: &str = "No time entries in this period";

pub const PROJ_REP_BACK: &str = "All projects";
pub const PROJ_REP_EDIT: &str = "Edit project";
pub const PROJ_REP_WHAT_WORKED_ON: &str = "What you worked on";
pub const PROJ_REP_FILES: &str = "Top windows & files";
pub const PROJ_REP_NO_DESC: &str = "No description";
