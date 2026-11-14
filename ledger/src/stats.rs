//! The beta's daily totals (M13, E12; owner, 15 Nov 2026: on by default for
//! testers). Counts only: what the ledger does, and what devices report with
//! their status (unless the person turned it off). Nothing is kept per device
//! or per account; the counts are added to the day's totals once a minute.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Mutex;

use mirrorleaf_proto::ledger::SyncMetrics;
use serde::Serialize;

/// Counts waiting to be added to the day's totals.
#[derive(Default)]
pub struct Counts(Mutex<HashMap<String, i64>>);

impl Counts {
    pub fn add(&self, name: impl Into<String>, n: i64) {
        if n != 0 {
            *self.0.lock().unwrap().entry(name.into()).or_default() += n;
        }
    }

    pub fn bump(&self, name: &str) {
        self.add(name, 1);
    }

    pub fn take(&self) -> Vec<(String, i64)> {
        self.0.lock().unwrap().drain().collect()
    }

    /// What a device counted since its last report.
    pub fn add_report(&self, m: &SyncMetrics) {
        for (name, n) in ARRIVAL_NAMES.iter().zip(m.arrivals) {
            self.add(*name, i64::from(n));
        }
        self.add("connections_direct", i64::from(m.direct));
        self.add("connections_relay", i64::from(m.relayed));
        let maker = maker(m.maker.as_deref());
        self.add(format!("woken_synced:{maker}"), i64::from(m.woken_synced));
        self.add(format!("woken_failed:{maker}"), i64::from(m.woken_failed));
    }
}

/// A table of the report: its heading, and rows of a label and one value
/// per day.
type Table = (&'static str, Vec<(&'static str, Vec<String>)>);

/// Names of the arrival bands, as in [`SyncMetrics::arrivals`].
pub const ARRIVAL_NAMES: [&str; 6] = [
    "arrived_under_5s",
    "arrived_under_30s",
    "arrived_under_5m",
    "arrived_under_1h",
    "arrived_under_24h",
    "arrived_later",
];

/// Phone makers counted by name; any other is "other", so a device can't add
/// names of its own to the totals.
const MAKERS: [&str; 22] = [
    "asus",
    "google",
    "honor",
    "huawei",
    "infinix",
    "iqoo",
    "lava",
    "lenovo",
    "motorola",
    "nokia",
    "nothing",
    "oneplus",
    "oppo",
    "poco",
    "realme",
    "redmi",
    "samsung",
    "sony",
    "tecno",
    "vivo",
    "xiaomi",
    "fairphone",
];

fn maker(reported: Option<&str>) -> &'static str {
    let Some(reported) = reported else {
        return "unknown";
    };
    let reported = reported.trim().to_ascii_lowercase();
    MAKERS
        .iter()
        .find(|m| reported == **m)
        .copied()
        .unwrap_or("other")
}

/// A version as reported, if it looks like one ("0.5.0").
pub fn version(reported: &str) -> Option<&str> {
    let parts: Vec<&str> = reported.split('.').collect();
    (reported.len() <= 16
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
    .then_some(reported)
}

/// The day number (days since 1970) for a Unix time.
pub fn day(now: i64) -> i32 {
    now.div_euclid(86_400) as i32
}

/// A day number as a date, "2026-11-15".
pub fn date(day: i32) -> String {
    // Howard Hinnant's days-to-civil.
    let z = i64::from(day) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Everything the dashboard and `stats` show.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub generated_at: i64,
    pub now: crate::db::Snapshot,
    pub versions: Vec<(String, i64)>,
    pub puzzle_bits: u32,
    pub sign_ups_last_hour: usize,
    /// Each day's totals, oldest first.
    pub days: Vec<(String, BTreeMap<String, i64>)>,
    /// The relay's own counters since it started, if it answered.
    pub relay: Vec<(String, f64)>,
}

impl Report {
    pub fn from_counts(rows: Vec<(i32, String, i64)>) -> Vec<(String, BTreeMap<String, i64>)> {
        let mut days: BTreeMap<i32, BTreeMap<String, i64>> = BTreeMap::new();
        for (day, name, value) in rows {
            days.entry(day).or_default().insert(name, value);
        }
        days.into_iter().map(|(d, c)| (date(d), c)).collect()
    }

    /// A day's figure, 0 when absent.
    fn get(day: &BTreeMap<String, i64>, name: &str) -> i64 {
        day.get(name).copied().unwrap_or(0)
    }

    /// The share of `part` in `whole`, as text.
    fn share(part: i64, whole: i64) -> String {
        if whole == 0 {
            "–".into()
        } else {
            format!("{:.0}%", part as f64 * 100.0 / whole as f64)
        }
    }

    /// Rows of the per-day tables: a heading and one value per day.
    fn rows(&self) -> Vec<Table> {
        let col = |f: &dyn Fn(&BTreeMap<String, i64>) -> String| -> Vec<String> {
            self.days.iter().map(|(_, d)| f(d)).collect()
        };
        let arrivals = |d: &BTreeMap<String, i64>| -> i64 {
            ARRIVAL_NAMES.iter().map(|n| Self::get(d, n)).sum()
        };
        let under = |d: &BTreeMap<String, i64>, bands: usize| -> i64 {
            ARRIVAL_NAMES[..bands].iter().map(|n| Self::get(d, n)).sum()
        };
        let sum_prefix = |d: &BTreeMap<String, i64>, prefix: &str| -> i64 {
            d.iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(_, v)| *v)
                .sum()
        };
        let caught = |d: &BTreeMap<String, i64>| -> i64 {
            [
                "caught_up_under_1h",
                "caught_up_under_24h",
                "caught_up_later",
            ]
            .iter()
            .map(|n| Self::get(d, n))
            .sum()
        };
        vec![
            (
                "Sync speed (changes arriving on another device)",
                vec![
                    ("Changes arrived", col(&|d| arrivals(d).to_string())),
                    (
                        "Within 5 s",
                        col(&|d| Self::share(under(d, 1), arrivals(d))),
                    ),
                    (
                        "Within 30 s",
                        col(&|d| Self::share(under(d, 2), arrivals(d))),
                    ),
                    (
                        "Within 1 h",
                        col(&|d| Self::share(under(d, 4), arrivals(d))),
                    ),
                    (
                        "Within 24 h",
                        col(&|d| Self::share(under(d, 5), arrivals(d))),
                    ),
                ],
            ),
            (
                "Devices catching up (a device behind until it has every edit)",
                vec![
                    ("Caught up", col(&|d| caught(d).to_string())),
                    (
                        "Within 1 h",
                        col(&|d| Self::share(Self::get(d, "caught_up_under_1h"), caught(d))),
                    ),
                    (
                        "Within 24 h",
                        col(&|d| {
                            Self::share(
                                Self::get(d, "caught_up_under_1h")
                                    + Self::get(d, "caught_up_under_24h"),
                                caught(d),
                            )
                        }),
                    ),
                ],
            ),
            (
                "Connections between devices",
                vec![
                    (
                        "Direct",
                        col(&|d| Self::get(d, "connections_direct").to_string()),
                    ),
                    (
                        "Through the relay",
                        col(&|d| Self::get(d, "connections_relay").to_string()),
                    ),
                    (
                        "Relay share",
                        col(&|d| {
                            let relay = Self::get(d, "connections_relay");
                            Self::share(relay, relay + Self::get(d, "connections_direct"))
                        }),
                    ),
                ],
            ),
            (
                "Waking phones",
                vec![
                    (
                        "Pushes sent",
                        col(&|d| sum_prefix(d, "push_sent").to_string()),
                    ),
                    (
                        "Pushes refused by Google",
                        col(&|d| Self::get(d, "push_failed").to_string()),
                    ),
                    (
                        "Woken and synced",
                        col(&|d| sum_prefix(d, "woken_synced").to_string()),
                    ),
                    (
                        "Woken, didn't sync",
                        col(&|d| sum_prefix(d, "woken_failed").to_string()),
                    ),
                    (
                        "\"Open Mirrorleaf on…\" prompts",
                        col(&|d| Self::get(d, "push_sent:fallback").to_string()),
                    ),
                ],
            ),
            (
                "Accounts and protection",
                vec![
                    (
                        "New accounts",
                        col(&|d| Self::get(d, "sign_ups").to_string()),
                    ),
                    (
                        "Devices added",
                        col(&|d| Self::get(d, "devices_added").to_string()),
                    ),
                    (
                        "Word-code lookups",
                        col(&|d| Self::get(d, "pairing_lookups").to_string()),
                    ),
                    (
                        "Relay passes",
                        col(&|d| Self::get(d, "relay_passes").to_string()),
                    ),
                    (
                        "Puzzle difficulty (bits)",
                        col(&|d| Self::get(d, "puzzle_bits").to_string()),
                    ),
                    (
                        "Refused: puzzle",
                        col(&|d| Self::get(d, "refused_puzzle").to_string()),
                    ),
                    (
                        "Refused: limits",
                        col(&|d| {
                            (Self::get(d, "refused_ip") + Self::get(d, "refused_device"))
                                .to_string()
                        }),
                    ),
                    (
                        "Refused: replayed",
                        col(&|d| Self::get(d, "refused_replay").to_string()),
                    ),
                    (
                        "Relay checks: in / out",
                        col(&|d| {
                            format!(
                                "{} / {}",
                                Self::get(d, "relay_allowed"),
                                Self::get(d, "relay_denied")
                            )
                        }),
                    ),
                ],
            ),
        ]
    }

    /// Wake-ups by phone maker over every day shown: (maker, synced, failed).
    fn makers(&self) -> Vec<(String, i64, i64)> {
        let mut by: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for (_, day) in &self.days {
            for (name, v) in day {
                if let Some(m) = name.strip_prefix("woken_synced:") {
                    by.entry(m.to_owned()).or_default().0 += v;
                } else if let Some(m) = name.strip_prefix("woken_failed:") {
                    by.entry(m.to_owned()).or_default().1 += v;
                }
            }
        }
        by.into_iter().map(|(m, (s, f))| (m, s, f)).collect()
    }

    /// The report as plain text, for `mirrorleaf-ledger stats`.
    pub fn text(&self) -> String {
        let n = &self.now;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Now: {} accounts ({} not used yet), {} devices ({}), {} active today, {} this week, {} behind, {} can be woken, {} blocked accounts",
            n.accounts,
            n.unused_accounts,
            n.devices,
            n.devices_by_platform
                .iter()
                .map(|(p, c)| format!("{p} {c}"))
                .collect::<Vec<_>>()
                .join(", "),
            n.active_today,
            n.active_week,
            n.behind,
            n.with_push,
            n.blocked
        );
        let _ = writeln!(
            out,
            "Versions: {}",
            self.versions
                .iter()
                .map(|(v, c)| format!("{v} {c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // The puzzle's live difficulty is in the running service's memory:
        // the dashboard shows it; here the daily figure below stands in.
        let dates: Vec<&str> = self.days.iter().map(|(d, _)| &d[5..]).collect();
        for (heading, rows) in self.rows() {
            let _ = writeln!(out, "\n{heading}");
            let _ = writeln!(out, "{:<34}{}", "", cells(&dates));
            for (label, values) in rows {
                let _ = writeln!(out, "{label:<34}{}", cells(&values));
            }
        }
        let makers = self.makers();
        if !makers.is_empty() {
            let _ = writeln!(out, "\nWake-ups by maker (synced / didn't)");
            for (m, s, f) in makers {
                let _ = writeln!(out, "{m:<34}{s} / {f}");
            }
        }
        if !self.relay.is_empty() {
            let _ = writeln!(out, "\nRelay counters since it started");
            for (name, v) in &self.relay {
                let _ = writeln!(out, "{name:<50}{v}");
            }
        }
        out
    }

    /// The report as a web page, for the dashboard.
    pub fn html(&self) -> String {
        let n = &self.now;
        let mut page = String::from(PAGE_HEAD);
        let _ = write!(
            page,
            "<header><h1>Mirrorleaf ledger</h1><p>Daily totals, no data about any one person. Updated {}.</p></header>",
            esc(&time(self.generated_at))
        );
        page.push_str("<section class=tiles>");
        let platforms = n
            .devices_by_platform
            .iter()
            .map(|(p, c)| format!("{c} {}", esc(p)))
            .collect::<Vec<_>>()
            .join(" · ");
        for (label, value, note) in [
            (
                "Accounts",
                n.accounts.to_string(),
                format!("{} not used yet", n.unused_accounts),
            ),
            ("Devices", n.devices.to_string(), platforms),
            (
                "Active today",
                n.active_today.to_string(),
                format!("{} this week", n.active_week),
            ),
            (
                "Behind now",
                n.behind.to_string(),
                format!("{} can be woken by push", n.with_push),
            ),
            (
                "Puzzle",
                format!("{} bits", self.puzzle_bits),
                format!("{} sign-ups in the last hour", self.sign_ups_last_hour),
            ),
            ("Blocked accounts", n.blocked.to_string(), String::new()),
        ] {
            let _ = write!(
                page,
                "<div class=tile><span>{}</span><strong>{}</strong><small>{}</small></div>",
                esc(label),
                esc(&value),
                esc(&note)
            );
        }
        page.push_str("</section>");
        let dates: Vec<String> = self.days.iter().map(|(d, _)| d[5..].to_owned()).collect();
        for (heading, rows) in self.rows() {
            let _ = write!(
                page,
                "<section><h2>{}</h2><div class=scroll><table><thead><tr><th></th>",
                esc(heading)
            );
            for d in &dates {
                let _ = write!(page, "<th>{}</th>", esc(d));
            }
            page.push_str("</tr></thead><tbody>");
            for (label, values) in rows {
                let _ = write!(page, "<tr><th>{}</th>", esc(label));
                for v in values {
                    let _ = write!(page, "<td>{}</td>", esc(&v));
                }
                page.push_str("</tr>");
            }
            page.push_str("</tbody></table></div></section>");
        }
        page.push_str("<section class=pair><div><h2>Wake-ups by phone maker</h2><table><thead><tr><th>Maker</th><th>Synced</th><th>Didn't</th><th>Share</th></tr></thead><tbody>");
        for (m, s, f) in self.makers() {
            let _ = write!(
                page,
                "<tr><th>{}</th><td>{s}</td><td>{f}</td><td>{}</td></tr>",
                esc(&m),
                Self::share(s, s + f)
            );
        }
        page.push_str("</tbody></table></div><div><h2>Devices by version</h2><table><tbody>");
        for (v, c) in &self.versions {
            let _ = write!(page, "<tr><th>{}</th><td>{c}</td></tr>", esc(v));
        }
        page.push_str("</tbody></table></div></section>");
        if !self.relay.is_empty() {
            page.push_str(
                "<section><h2>Relay counters since it started</h2><div class=scroll><table><tbody>",
            );
            for (name, v) in &self.relay {
                let _ = write!(page, "<tr><th>{}</th><td>{v}</td></tr>", esc(name));
            }
            page.push_str("</tbody></table></div></section>");
        }
        page.push_str("<footer>Google's own push delivery figures: Firebase console → Cloud Messaging → Reports.</footer></main>");
        page
    }
}

fn cells(values: &[impl AsRef<str>]) -> String {
    values
        .iter()
        .map(|v| format!("{:>8}", v.as_ref()))
        .collect::<String>()
}

fn time(unix: i64) -> String {
    let secs = unix.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02} UTC",
        date(day(unix)),
        secs / 3600,
        secs / 60 % 60
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Reads the relay's counters (Prometheus text): the lines without labels.
pub fn relay_counters(text: &str) -> Vec<(String, f64)> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.contains('{'))
        .filter_map(|l| {
            let (name, value) = l.split_once(' ')?;
            Some((name.to_owned(), value.trim().parse().ok()?))
        })
        .collect()
}

const PAGE_HEAD: &str = r#"<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>Mirrorleaf ledger</title>
<style>
:root{--bg:#f6f3ec;--card:#fffdf8;--ink:#2b2620;--muted:#6f675c;--line:#e2dccf;--accent:#4e6a35;color-scheme:light}
@media (prefers-color-scheme:dark){:root{--bg:#1b1a17;--card:#24221e;--ink:#ece6da;--muted:#a59d8f;--line:#3a362f;--accent:#9cbf7a;color-scheme:dark}}
body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.45 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1100px;margin:0 auto;padding:24px max(16px,env(safe-area-inset-left))}
h1{font-size:22px;margin:0 0 4px}h2{font-size:15px;margin:0 0 10px}header p,small,footer{color:var(--muted)}
section{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:16px;margin:16px 0}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px;background:none;border:0;padding:0}
.tile{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px;display:flex;flex-direction:column;gap:2px}
.tile span{color:var(--muted);font-size:12px;text-transform:uppercase;letter-spacing:.04em}.tile strong{font-size:24px;color:var(--accent);font-variant-numeric:tabular-nums}
.scroll{overflow-x:auto}table{border-collapse:collapse;font-variant-numeric:tabular-nums;width:100%}
th,td{padding:5px 8px;border-bottom:1px solid var(--line);text-align:right;white-space:nowrap}
tbody th{text-align:left;font-weight:500}thead th{color:var(--muted);font-weight:500}
.pair{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:24px}
</style><main>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_makers_and_versions() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(day(1_794_700_800)), "2026-11-15");
        assert_eq!(maker(Some("Motorola")), "motorola");
        assert_eq!(maker(Some("<script>")), "other");
        assert_eq!(maker(None), "unknown");
        assert_eq!(version("0.5.0"), Some("0.5.0"));
        assert_eq!(version("0.5.0-evil"), None);
        assert_eq!(
            relay_counters("# HELP x\nrelay_bytes_sent 12\nrelay_x{a=\"b\"} 3\n"),
            vec![("relay_bytes_sent".into(), 12.0)]
        );
    }

    #[test]
    fn a_report_counts_bands_and_makers() {
        let counts = Counts::default();
        let mut m = SyncMetrics::default();
        m.arrived_after(2);
        m.arrived_after(40);
        m.woken_synced = 3;
        m.maker = Some("samsung".into());
        counts.add_report(&m);
        let mut taken = counts.take();
        taken.sort();
        assert!(taken.contains(&("arrived_under_5s".into(), 1)));
        assert!(taken.contains(&("arrived_under_5m".into(), 1)));
        assert!(taken.contains(&("woken_synced:samsung".into(), 3)));
        assert!(counts.take().is_empty());
    }
}
