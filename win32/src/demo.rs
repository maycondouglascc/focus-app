//! Deterministic in-memory demo dataset (`--demo`), ported from Electron
//! `demoApi.ts` and `demoClock.ts`. Seeds 156 weeks (3 years) of projects,
//! time entries, app activity, and Claude Code sessions with the clock frozen
//! at Friday 15:47:00 so the live timer is always 0:47:00.

use std::collections::{HashMap, HashSet};

use crate::db::{
    system_now_ms, Activity, CurrentActivity, EntryInput, EntryPatch, MonitorStatus, Project,
    ProjectAllTimeStats, ProjectInput, ProjectPatch, ProjectSummary, Settings, SettingsPatch,
    Store, TimeEntry, TimerStartInput,
};
use crate::locale::SystemLocale;
use crate::stats::{earned, entry_ms, union_ms_sorted};
use crate::time::{
    add_days, add_days_start, at_minutes, overlap, start_of_day, start_of_week, to_local, MIN,
};

struct Rng(u64);

impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 = (self.0 * 16_807) % 2_147_483_647;
        (self.0 as f64) / 2_147_483_647.0
    }

    fn pick_idx(&mut self, len: usize) -> usize {
        ((self.next_f64() * (len as f64)).floor() as usize).min(len.saturating_sub(1))
    }

    fn pick_weighted<'a, T>(&mut self, items: &'a [(T, f64)]) -> &'a T {
        let sum: f64 = items.iter().map(|(_, w)| *w).sum();
        let mut r = self.next_f64() * sum;
        for (item, w) in items {
            r -= *w;
            if r <= 0.0 {
                return item;
            }
        }
        &items.last().expect("non-empty weighted items").0
    }
}

struct TaskDef {
    label: &'static str,
    weight: f64,
    apps: &'static [(&'static str, &'static str, &'static str, &'static str)],
    claude: Option<(&'static str, &'static str)>,
}

/// Freezes demo "now" at Friday 15:47:00 of the current Monday-based week.
pub fn demo_frozen_now(real_now_ms: i64) -> i64 {
    let mon = start_of_week(real_now_ms);
    let fri = add_days(mon, 4);
    at_minutes(fri, 15 * 60 + 47)
}

pub struct DemoStore {
    frozen_now: i64,
    locale: SystemLocale,
    projects: Vec<Project>,
    entries: Vec<TimeEntry>,
    activity: Vec<Activity>,
    settings: Settings,
    next_uid: u64,
}

impl DemoStore {
    pub fn new(locale: SystemLocale) -> Self {
        Self::build(demo_frozen_now(system_now_ms()), locale, false)
    }

    pub fn fresh(locale: SystemLocale) -> Self {
        Self::build(demo_frozen_now(system_now_ms()), locale, true)
    }

    fn alloc_id(&mut self, prefix: &str) -> String {
        self.next_uid += 1;
        format!("{prefix}-{}", self.next_uid)
    }

    fn build(now: i64, locale: SystemLocale, fresh: bool) -> Self {
        let mut settings = Settings::defaults_for(&locale);
        settings.currency = "USD".into();
        settings.onboarded = !fresh;

        if fresh {
            return Self {
                frozen_now: now,
                locale,
                projects: Vec::new(),
                entries: Vec::new(),
                activity: Vec::new(),
                settings,
                next_uid: 0,
            };
        }

        let projects = vec![
            Project {
                id: "p1".into(),
                name: "Northwind".into(),
                client: "Northwind Traders".into(),
                color: "#d9488a".into(),
                rate: 140.0,
                currency: "USD".into(),
                billable_default: true,
                archived: false,
                created_at: 1,
            },
            Project {
                id: "p2".into(),
                name: "Halcyon app".into(),
                client: "Halcyon Health".into(),
                color: "#3b7ddd".into(),
                rate: 125.0,
                currency: "USD".into(),
                billable_default: true,
                archived: false,
                created_at: 2,
            },
            Project {
                id: "p3".into(),
                name: "Atlas UI kit".into(),
                client: "Atlas".into(),
                color: "#2f9e8f".into(),
                rate: 150.0,
                currency: "USD".into(),
                billable_default: true,
                archived: false,
                created_at: 3,
            },
            Project {
                id: "p4".into(),
                name: "Orbit deck".into(),
                client: "Orbit Labs".into(),
                color: "#e08a2c".into(),
                rate: 110.0,
                currency: "USD".into(),
                billable_default: true,
                archived: false,
                created_at: 4,
            },
            Project {
                id: "p5".into(),
                name: "Studio admin".into(),
                client: String::new(),
                color: "#6c56d9".into(),
                rate: 0.0,
                currency: "USD".into(),
                billable_default: false,
                archived: false,
                created_at: 5,
            },
            Project {
                id: "p6".into(),
                name: "Lumen website".into(),
                client: "Lumen".into(),
                color: "#4aa3c7".into(),
                rate: 95.0,
                currency: "USD".into(),
                billable_default: true,
                archived: true,
                created_at: 0,
            },
            Project {
                id: "p7".into(),
                name: "Pine banking".into(),
                client: "Pine Bank".into(),
                color: "#5a8f3c".into(),
                rate: 115.0,
                currency: "USD".into(),
                billable_default: true,
                archived: true,
                created_at: 0,
            },
        ];

        let tasks_map = demo_tasks();
        let chatter: &[(&str, &str, &str, &str)] = &[
            ("Slack", "northwind-design", "", ""),
            ("Slack", "halcyon-dev", "", ""),
            ("Mail", "Inbox", "", ""),
            ("Messages", "", "", ""),
            ("Edge", "Home / X", "x.com", "https://x.com/home"),
            ("Linear", "My issues", "", ""),
        ];

        let eras: [(&str, f64, i32, i32); 7] = [
            ("p6", 5.0, 156, 92),
            ("p7", 5.0, 118, 48),
            ("p3", 3.0, 70, 0),
            ("p2", 5.0, 56, 0),
            ("p1", 4.0, 22, 0),
            ("p4", 2.0, 9, 0),
            ("p5", 1.5, 156, 0),
        ];

        let mut rng = Rng(11);
        let mut entries = Vec::new();
        let mut activity = Vec::new();
        let mut next_uid = 0u64;

        let push_act = |activity: &mut Vec<Activity>,
                        kind: &str,
                        app: &str,
                        bundle_id: &str,
                        detail: &str,
                        context: &str,
                        url: &str,
                        start: i64,
                        end: i64| {
            let end = end.min(now);
            if end - start >= MIN {
                let id = (activity.len() + 1) as i64;
                activity.push(Activity {
                    id,
                    kind: kind.into(),
                    app: app.into(),
                    bundle_id: bundle_id.into(),
                    detail: detail.into(),
                    context: context.into(),
                    url: url.into(),
                    start,
                    end,
                });
            }
        };

        let fill = |rng: &mut Rng,
                    activity: &mut Vec<Activity>,
                    from: i64,
                    to: i64,
                    wins: &[(&str, &str, &str, &str)],
                    coarse: bool| {
            let mut t = from;
            while t < to && t < now {
                let len = if coarse {
                    (25 + (rng.next_f64() * 35.0).floor() as i64) * MIN
                } else {
                    (3 + (rng.next_f64() * 18.0).floor() as i64) * MIN
                };
                let (app, detail, context, url) = if rng.next_f64() < 0.78 {
                    wins[rng.pick_idx(wins.len())]
                } else {
                    chatter[rng.pick_idx(chatter.len())]
                };
                push_act(
                    activity,
                    "app",
                    app,
                    "",
                    detail,
                    context,
                    url,
                    t,
                    (t + len).min(to),
                );
                let r = rng.next_f64();
                let pause = if r < 0.1 {
                    (1 + (rng.next_f64() * 3.0).floor() as i64) * MIN
                } else if r < 0.2 {
                    (7 + (rng.next_f64() * 8.0).floor() as i64) * MIN
                } else {
                    0
                };
                t += len + pause;
            }
        };

        const WEEKS: i32 = 156;
        let week0 = add_days(start_of_week(now), -WEEKS * 7);
        let mut holidays = HashSet::new();
        for w in 0..WEEKS {
            if rng.next_f64() < 0.05 {
                holidays.insert(w);
            }
        }
        let today_start = start_of_day(now);
        let lengths = [45i32, 60, 75, 90, 90, 105, 120, 150];
        let gaps = [0i32, 0, 15, 15, 30];
        let script: [(&str, usize, i32); 5] = [
            ("p2", 1, 90),
            ("p1", 0, 90),
            ("p3", 1, 90),
            ("p5", 1, 30),
            ("p2", 0, 120),
        ];

        for d in 0..(WEEKS + 1) * 7 {
            let day = add_days_start(week0, d);
            let dow = to_local(day).weekday_mon0();
            let weeks_ago = WEEKS - d / 7;
            if dow > 4
                || day > now
                || (weeks_ago > 0 && holidays.contains(&weeks_ago))
                || rng.next_f64() < 0.05
            {
                continue;
            }
            let weights: Vec<(&str, f64)> = eras
                .iter()
                .filter(|(_, _, a, b)| weeks_ago <= *a && weeks_ago >= *b)
                .map(|(id, w, _, _)| (*id, *w))
                .collect();
            let coarse = weeks_ago > 16;
            let mut m = 9 * 60 + ((rng.next_f64() * 3.0).floor() as i32) * 15;
            fill(
                &mut rng,
                &mut activity,
                at_minutes(day, m - 20),
                at_minutes(day, m),
                chatter,
                coarse,
            );
            let is_today = day == today_start;
            let mut step = 0usize;
            if is_today {
                m = 9 * 60;
            }
            let mut prev = String::new();
            let mut lunch = false;
            while m < 18 * 60 {
                if !lunch && m >= 12 * 60 + 15 {
                    lunch = true;
                    m += 45;
                    continue;
                }
                let scripted = if is_today && step < script.len() {
                    let s = script[step];
                    step += 1;
                    Some(s)
                } else {
                    None
                };
                let mut project_id = scripted
                    .map(|s| s.0.to_string())
                    .unwrap_or_else(|| (*rng.pick_weighted(&weights)).to_string());
                if scripted.is_none() && project_id == prev {
                    project_id = (*rng.pick_weighted(&weights)).to_string();
                }
                prev = project_id.clone();
                let proj = projects.iter().find(|x| x.id == project_id).unwrap();
                let task_list = &tasks_map[project_id.as_str()];
                let task = if let Some(sc) = scripted {
                    &task_list[sc.1]
                } else {
                    let weighted: Vec<(&TaskDef, f64)> =
                        task_list.iter().map(|t| (t, t.weight)).collect();
                    *rng.pick_weighted(&weighted)
                };
                let len = scripted
                    .map(|s| s.2)
                    .unwrap_or_else(|| lengths[rng.pick_idx(lengths.len())]);
                let start = at_minutes(day, m);
                let end = at_minutes(day, (m + len).min(18 * 60 + 30));
                if start >= now {
                    break;
                }
                let running = end > now;
                next_uid += 1;
                entries.push(TimeEntry {
                    id: format!("e-{next_uid}"),
                    project_id: Some(project_id.clone()),
                    description: task.label.into(),
                    start,
                    end: if running { None } else { Some(end) },
                    billable: proj.billable_default,
                    source: if running {
                        "timer".into()
                    } else {
                        "calendar".into()
                    },
                });
                fill(&mut rng, &mut activity, start, end, task.apps, coarse);
                if let Some((ctitle, cfolder)) = task.claude {
                    let cs = start + ((rng.next_f64() * 15.0).floor() as i64) * MIN;
                    let cend = (cs + (25 + (rng.next_f64() * 40.0).floor() as i64) * MIN).min(end);
                    push_act(
                        &mut activity,
                        "claude",
                        "Claude Code",
                        "claude-desktop",
                        ctitle,
                        cfolder,
                        "",
                        cs,
                        cend,
                    );
                }
                if running {
                    break;
                }
                let gap = if is_today {
                    if step == 1 {
                        15
                    } else {
                        0
                    }
                } else {
                    gaps[rng.pick_idx(gaps.len())]
                };
                if gap > 0 {
                    fill(
                        &mut rng,
                        &mut activity,
                        end,
                        at_minutes(day, m + len + gap),
                        chatter,
                        coarse,
                    );
                }
                m += len + gap;
            }
        }

        Self {
            frozen_now: now,
            locale,
            projects,
            entries,
            activity,
            settings,
            next_uid,
        }
    }
}

fn demo_tasks() -> HashMap<&'static str, Vec<TaskDef>> {
    let mut m = HashMap::new();
    m.insert(
        "p1",
        vec![
            TaskDef {
                label: "Logo exploration",
                weight: 3.0,
                apps: &[
                    ("Figma", "Northwind — Logo v4", "", ""),
                    ("Figma", "Northwind — Moodboard", "", ""),
                    (
                        "Edge",
                        "Brand New — Logo reviews",
                        "underconsideration.com",
                        "https://www.underconsideration.com/brandnew/",
                    ),
                ],
                claude: None,
            },
            TaskDef {
                label: "Brand guidelines",
                weight: 2.0,
                apps: &[
                    ("Figma", "Northwind — Guidelines", "", ""),
                    ("Notion", "Northwind · Voice and tone", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Website hero",
                weight: 2.0,
                apps: &[
                    ("Figma", "Northwind — Website", "", ""),
                    ("Framer", "northwind.co — Home", "", ""),
                    (
                        "Edge",
                        "Northwind — Staging",
                        "northwind.framer.website",
                        "https://northwind.framer.website",
                    ),
                ],
                claude: None,
            },
            TaskDef {
                label: "Client review call",
                weight: 1.0,
                apps: &[
                    ("Zoom", "Northwind weekly", "", ""),
                    ("Figma", "Northwind — Logo v4", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m.insert(
        "p2",
        vec![
            TaskDef {
                label: "Onboarding flow",
                weight: 3.0,
                apps: &[
                    ("Figma", "Halcyon — Onboarding", "", ""),
                    ("Figma", "Halcyon — Prototype", "", ""),
                    ("Linear", "HAL-212 Onboarding copy", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Settings screen",
                weight: 3.0,
                apps: &[
                    (
                        "Visual Studio Code",
                        "SettingsScreen.tsx",
                        "halcyon-app",
                        "",
                    ),
                    ("Windows Terminal", "cargo test", "", ""),
                    (
                        "Edge",
                        "React Native docs",
                        "reactnative.dev",
                        "https://reactnative.dev/docs",
                    ),
                ],
                claude: Some(("Settings screen with sections", "halcyon-app")),
            },
            TaskDef {
                label: "Push notifications fix",
                weight: 2.0,
                apps: &[
                    ("Visual Studio Code", "notifications.ts", "halcyon-app", ""),
                    (
                        "Edge",
                        "Web Push API docs",
                        "developer.mozilla.org",
                        "https://developer.mozilla.org",
                    ),
                ],
                claude: Some(("Debug background push handler", "halcyon-app")),
            },
            TaskDef {
                label: "Sprint planning",
                weight: 1.0,
                apps: &[
                    ("Linear", "Halcyon · Sprint 14", "", ""),
                    ("Zoom", "Halcyon planning", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m.insert(
        "p3",
        vec![
            TaskDef {
                label: "Button and input tokens",
                weight: 3.0,
                apps: &[
                    ("Figma", "Atlas — Components", "", ""),
                    ("Figma", "Atlas — Tokens", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Docs site",
                weight: 2.0,
                apps: &[
                    ("Visual Studio Code", "button.mdx", "atlas-docs", ""),
                    (
                        "Edge",
                        "Atlas Docs — Button",
                        "localhost:3000",
                        "http://localhost:3000/button",
                    ),
                ],
                claude: Some(("Generate token tables for docs", "atlas-docs")),
            },
            TaskDef {
                label: "Component audit",
                weight: 1.0,
                apps: &[
                    ("Notion", "Atlas · Component audit", "", ""),
                    ("Figma", "Atlas — Audit", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m.insert(
        "p4",
        vec![
            TaskDef {
                label: "Narrative outline",
                weight: 2.0,
                apps: &[
                    ("Notion", "Orbit · Pitch narrative", "", ""),
                    ("Edge", "Orbit Labs", "orbitlabs.io", "https://orbitlabs.io"),
                ],
                claude: None,
            },
            TaskDef {
                label: "Seed deck slides",
                weight: 3.0,
                apps: &[
                    ("PowerPoint", "Orbit Seed Deck", "", ""),
                    ("Figma", "Orbit — Charts", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Market sizing",
                weight: 1.0,
                apps: &[
                    (
                        "Edge",
                        "Crunchbase — Climate analytics",
                        "crunchbase.com",
                        "https://www.crunchbase.com",
                    ),
                    ("Excel", "Orbit TAM", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m.insert(
        "p5",
        vec![
            TaskDef {
                label: "Invoices and bookkeeping",
                weight: 2.0,
                apps: &[
                    (
                        "Edge",
                        "Stripe — Invoices",
                        "dashboard.stripe.com",
                        "https://dashboard.stripe.com/invoices",
                    ),
                    ("Excel", "Studio 2026", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Inbox",
                weight: 3.0,
                apps: &[
                    ("Mail", "Inbox", "", ""),
                    ("Slack", "studio-general", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Portfolio case study",
                weight: 1.0,
                apps: &[
                    ("Framer", "Portfolio — Halcyon case study", "", ""),
                    ("Figma", "Portfolio shots", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m.insert(
        "p6",
        vec![
            TaskDef {
                label: "Homepage design",
                weight: 3.0,
                apps: &[
                    ("Figma", "Lumen — Homepage", "", ""),
                    (
                        "Edge",
                        "Lumen staging",
                        "lumen.webflow.io",
                        "https://lumen.webflow.io",
                    ),
                ],
                claude: None,
            },
            TaskDef {
                label: "Webflow build",
                weight: 2.0,
                apps: &[
                    (
                        "Edge",
                        "Webflow — Lumen",
                        "webflow.com",
                        "https://webflow.com",
                    ),
                    ("Figma", "Lumen — Components", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m.insert(
        "p7",
        vec![
            TaskDef {
                label: "Account overview screens",
                weight: 3.0,
                apps: &[
                    ("Figma", "Pine — Accounts", "", ""),
                    ("Figma", "Pine — Prototype", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Usability testing",
                weight: 1.0,
                apps: &[
                    ("Zoom", "Pine research session", "", ""),
                    ("Notion", "Pine · Test notes", "", ""),
                ],
                claude: None,
            },
            TaskDef {
                label: "Design QA",
                weight: 2.0,
                apps: &[
                    (
                        "Edge",
                        "Pine TestFlight notes",
                        "pine.atlassian.net",
                        "https://pine.atlassian.net",
                    ),
                    ("Figma", "Pine — Accounts", "", ""),
                ],
                claude: None,
            },
        ],
    );
    m
}

impl Store for DemoStore {
    fn now_ms(&self) -> i64 {
        self.frozen_now
    }

    fn is_demo(&self) -> bool {
        true
    }

    fn system_locale(&self) -> &SystemLocale {
        &self.locale
    }

    fn list_projects(&self) -> Result<Vec<Project>, String> {
        let mut out = self.projects.clone();
        out.sort_by_key(|p| (p.archived, p.created_at));
        Ok(out)
    }

    fn get_project(&self, id: &str) -> Result<Project, String> {
        self.projects
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| "Project not found".into())
    }

    fn create_project(&mut self, input: ProjectInput) -> Result<Project, String> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err("Give the project a name.".into());
        }
        let id = self.alloc_id("p");
        let p = Project {
            id,
            name: name.into(),
            client: input.client.trim().into(),
            color: input.color,
            rate: input.rate.max(0.0),
            currency: input.currency,
            billable_default: input.billable_default,
            archived: input.archived,
            created_at: self.frozen_now,
        };
        self.projects.push(p.clone());
        Ok(p)
    }

    fn update_project(&mut self, id: &str, patch: ProjectPatch) -> Result<Project, String> {
        let p = self
            .projects
            .iter_mut()
            .find(|x| x.id == id)
            .ok_or_else(|| "Project not found".to_string())?;
        if let Some(v) = patch.name {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                return Err("Give the project a name.".into());
            }
            p.name = trimmed.into();
        }
        if let Some(v) = patch.client {
            p.client = v.trim().into();
        }
        if let Some(v) = patch.color {
            p.color = v;
        }
        if let Some(v) = patch.rate {
            p.rate = v.max(0.0);
        }
        if let Some(v) = patch.currency {
            p.currency = v;
        }
        if let Some(v) = patch.billable_default {
            p.billable_default = v;
        }
        if let Some(v) = patch.archived {
            p.archived = v;
        }
        Ok(p.clone())
    }

    fn delete_project(&mut self, id: &str) -> Result<(), String> {
        let pos = self
            .projects
            .iter()
            .position(|x| x.id == id)
            .ok_or_else(|| "Project not found".to_string())?;
        self.projects.remove(pos);
        for e in &mut self.entries {
            if e.project_id.as_deref() == Some(id) {
                e.project_id = None;
            }
        }
        Ok(())
    }

    fn project_summaries(
        &self,
        month_from: i64,
        month_to: i64,
    ) -> Result<HashMap<String, ProjectSummary>, String> {
        let by_id: HashMap<&str, &Project> =
            self.projects.iter().map(|p| (p.id.as_str(), p)).collect();
        let mut out: HashMap<String, ProjectSummary> = HashMap::new();
        for e in &self.entries {
            let Some(pid) = e.project_id.as_deref() else {
                continue;
            };
            let proj = by_id.get(pid).copied();
            let s = out.entry(pid.into()).or_default();
            s.total_ms += entry_ms(e, i64::MIN, i64::MAX, self.frozen_now);
            s.month_ms += entry_ms(e, month_from, month_to, self.frozen_now);
            s.earned += earned(e, proj, i64::MIN, i64::MAX, self.frozen_now);
        }
        Ok(out)
    }

    fn project_all_time_stats(&self, project_id: &str) -> Result<ProjectAllTimeStats, String> {
        let proj = self.projects.iter().find(|p| p.id == project_id);
        let mut out = ProjectAllTimeStats::default();
        for e in &self.entries {
            if e.project_id.as_deref() != Some(project_id) {
                continue;
            }
            out.first_start = Some(out.first_start.map_or(e.start, |m| m.min(e.start)));
            out.total_ms += entry_ms(e, i64::MIN, i64::MAX, self.frozen_now);
            out.earned += earned(e, proj, i64::MIN, i64::MAX, self.frozen_now);
            out.count += 1;
        }
        Ok(out)
    }

    fn list_entries(&self, from: i64, to: i64) -> Result<Vec<TimeEntry>, String> {
        let mut out: Vec<TimeEntry> = self
            .entries
            .iter()
            .filter(|e| e.start < to && e.end.unwrap_or(self.frozen_now) > from)
            .cloned()
            .collect();
        out.sort_by_key(|e| e.start);
        Ok(out)
    }

    fn get_entry(&self, id: &str) -> Result<TimeEntry, String> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .ok_or_else(|| "Entry not found".to_string())
    }

    fn has_any_entries(&self) -> Result<bool, String> {
        Ok(!self.entries.is_empty())
    }

    fn earliest_entry_start(
        &self,
        project_id: Option<&str>,
        no_project_only: bool,
    ) -> Result<Option<i64>, String> {
        Ok(self
            .entries
            .iter()
            .filter(|e| {
                if no_project_only {
                    e.project_id.is_none()
                } else if let Some(pid) = project_id {
                    e.project_id.as_deref() == Some(pid)
                } else {
                    true
                }
            })
            .map(|e| e.start)
            .min())
    }

    fn create_entry(&mut self, input: EntryInput) -> Result<TimeEntry, String> {
        if let Some(end) = input.end {
            if end <= input.start {
                return Err("End must be after start".into());
            }
        }
        let e = TimeEntry {
            id: self.alloc_id("e"),
            project_id: input.project_id,
            description: input.description.trim().into(),
            start: input.start,
            end: input.end,
            billable: input.billable,
            source: input.source,
        };
        self.entries.push(e.clone());
        Ok(e)
    }

    fn update_entry(&mut self, id: &str, patch: EntryPatch) -> Result<TimeEntry, String> {
        let e = self
            .entries
            .iter_mut()
            .find(|x| x.id == id)
            .ok_or_else(|| "Entry not found".to_string())?;
        let mut next = e.clone();
        if let Some(pid) = patch.project_id {
            next.project_id = pid;
        }
        if let Some(d) = patch.description {
            next.description = d.trim().into();
        }
        if let Some(s) = patch.start {
            next.start = s;
        }
        if let Some(end) = patch.end {
            next.end = end;
        }
        if let Some(b) = patch.billable {
            next.billable = b;
        }
        if let Some(src) = patch.source {
            next.source = src;
        }
        if let Some(end) = next.end {
            if end <= next.start {
                return Err("End must be after start".into());
            }
        }
        *e = next.clone();
        Ok(next)
    }

    fn delete_entry(&mut self, id: &str) -> Result<(), String> {
        if let Some(pos) = self.entries.iter().position(|x| x.id == id) {
            self.entries.remove(pos);
        }
        Ok(())
    }

    fn running_entry(&self) -> Result<Option<TimeEntry>, String> {
        Ok(self.entries.iter().rev().find(|e| e.end.is_none()).cloned())
    }

    fn start_timer(&mut self, input: TimerStartInput) -> Result<TimeEntry, String> {
        let _ = self.stop_timer()?;
        self.create_entry(EntryInput {
            project_id: input.project_id,
            description: input.description,
            start: self.frozen_now,
            end: None,
            billable: input.billable,
            source: "timer".into(),
        })
    }

    fn stop_timer(&mut self) -> Result<Option<TimeEntry>, String> {
        let now = self.frozen_now;
        let Some(pos) = self.entries.iter().rposition(|e| e.end.is_none()) else {
            return Ok(None);
        };
        let start = self.entries[pos].start;
        let end = now.max(start + 1000);
        for e in &mut self.entries {
            if e.end.is_none() {
                e.end = Some(end);
            }
        }
        let target_id = self.entries[pos].id.clone();
        self.entries
            .retain(|e| !(e.source == "timer" && e.end.is_some_and(|x| x - e.start < 60_000)));
        Ok(self.entries.iter().find(|e| e.id == target_id).cloned())
    }

    fn list_activity(&self, from: i64, to: i64) -> Result<Vec<Activity>, String> {
        Ok(self
            .activity
            .iter()
            .filter(|a| overlap(a.start, a.end, from, to) > 0)
            .cloned()
            .collect())
    }

    fn visit_activity(
        &self,
        from: i64,
        to: i64,
        f: &mut dyn FnMut(Activity),
    ) -> Result<(), String> {
        for a in &self.activity {
            if overlap(a.start, a.end, from, to) > 0 {
                f(a.clone());
            }
        }
        Ok(())
    }

    fn active_union_ms(&self, from: i64, to: i64) -> Result<i64, String> {
        let mut iv: Vec<(i64, i64)> = self
            .activity
            .iter()
            .filter(|a| a.kind == "app")
            .filter_map(|a| {
                let s = a.start.max(from);
                let e = a.end.min(to);
                (e > s).then_some((s, e))
            })
            .collect();
        iv.sort_unstable_by_key(|x| (x.0, x.1));
        Ok(union_ms_sorted(&iv))
    }

    fn earliest_activity_start(&self) -> Result<Option<i64>, String> {
        Ok(self.activity.iter().map(|a| a.start).min())
    }

    fn clear_activity(&mut self) -> Result<(), String> {
        self.activity.clear();
        Ok(())
    }

    fn get_settings(&self) -> Result<Settings, String> {
        Ok(self.settings.clone())
    }

    fn save_settings(&mut self, patch: SettingsPatch) -> Result<Settings, String> {
        if let Some(v) = patch.currency {
            self.settings.currency = v;
        }
        if let Some(v) = patch.monitor_enabled {
            self.settings.monitor_enabled = v;
        }
        if let Some(v) = patch.idle_minutes {
            self.settings.idle_minutes = v;
        }
        if let Some(v) = patch.browser_tabs {
            self.settings.browser_tabs = v;
        }
        if let Some(v) = patch.claude_sessions {
            self.settings.claude_sessions = v;
        }
        if let Some(v) = patch.day_start_hour {
            self.settings.day_start_hour = v;
        }
        if let Some(v) = patch.onboarded {
            self.settings.onboarded = v;
        }
        if let Some(v) = patch.first_day_of_week {
            self.settings.first_day_of_week = v;
        }
        if let Some(v) = patch.time_format {
            self.settings.time_format = v;
        }
        if let Some(v) = patch.keep_active_when_busy {
            self.settings.keep_active_when_busy = v;
        }
        if let Some(v) = patch.excluded_apps {
            self.settings.excluded_apps = v;
        }
        if let Some(v) = patch.excluded_title_patterns {
            self.settings.excluded_title_patterns = v;
        }
        if let Some(v) = patch.csv_standard {
            self.settings.csv_standard = v;
        }
        if let Some(v) = patch.mini_timer_enabled {
            self.settings.mini_timer_enabled = v;
        }
        if let Some(v) = patch.mini_timer_x {
            self.settings.mini_timer_x = v;
        }
        if let Some(v) = patch.mini_timer_y {
            self.settings.mini_timer_y = v;
        }
        if let Some(v) = patch.tray_hint_shown {
            self.settings.tray_hint_shown = v;
        }
        Ok(self.settings.clone())
    }

    fn monitor_status(&self) -> MonitorStatus {
        MonitorStatus {
            enabled: self.settings.monitor_enabled,
            idle: false,
            current: if self.settings.onboarded {
                Some(CurrentActivity {
                    app: "Figma".into(),
                    detail: "Halcyon — Onboarding".into(),
                    since: self.frozen_now - 12 * MIN,
                })
            } else {
                None
            },
            claude_last_scan: if self.settings.onboarded {
                Some(self.frozen_now)
            } else {
                None
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::clock;

    #[test]
    fn demo_freezes_at_friday_1547_with_47min_running_timer_and_3_years_data() {
        let store = DemoStore::new(SystemLocale::en_us());
        let now = store.now_ms();
        let l = to_local(now);
        assert_eq!(l.weekday_mon0(), 4); // Friday
        assert_eq!((l.hour, l.minute), (15, 47));

        let running = store.running_entry().unwrap().expect("running demo timer");
        assert_eq!(running.description, "Onboarding flow");
        assert_eq!(clock(now - running.start), "0:47:00");

        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 7);
        assert_eq!(projects.iter().filter(|p| p.archived).count(), 2);

        let week = start_of_week(now);
        let week_entries = store.list_entries(week, add_days(week, 7)).unwrap();
        assert!(!week_entries.is_empty());
        assert!(store.earliest_entry_start(None, false).unwrap().is_some());
        assert!(store.earliest_activity_start().unwrap().is_some());
    }

    #[test]
    fn demo_fresh_starts_empty_and_not_onboarded() {
        let mut store = DemoStore::fresh(SystemLocale::pt_br());
        assert!(!store.get_settings().unwrap().onboarded);
        assert!(store.list_projects().unwrap().is_empty());
        assert!(!store.has_any_entries().unwrap());

        let p = store
            .create_project(ProjectInput {
                name: "Test".into(),
                client: String::new(),
                color: "#3b7ddd".into(),
                rate: 100.0,
                currency: "BRL".into(),
                billable_default: true,
                archived: false,
            })
            .unwrap();
        assert_eq!(store.list_projects().unwrap().len(), 1);
        store.delete_project(&p.id).unwrap();
        assert!(store.list_projects().unwrap().is_empty());
    }
}
