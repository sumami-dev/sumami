//! The assistant: ask a question, get an answer.
//!
//! The answer is **computed, not written**. The question is turned into a
//! report — the same report the dashboard asks for — the report is run through
//! the same code the API runs, and the answer is written from the numbers that
//! came back.
//!
//! That has three consequences worth having:
//!
//! * **the assistant cannot disagree with the dashboard.** There is one
//!   implementation of "the top pages" and one implementation of "page views",
//!   and the assistant uses both;
//! * **it works with no key, no network and no data leaving the machine.** The
//!   privacy promise does not get a hole in it for a chat window;
//! * **it cannot make up a number.** Every figure in the answer is a figure the
//!   database returned.
//!
//! What it is not: a language model. It reads a question, and it answers with
//! the numbers it found. A language model can be put in front of it (the
//! [`Facts`] are plain JSON, and a model can be asked to phrase them) — but the
//! model would only ever be rearranging what the database said.
//!
//! The questions it understands are the ones a site owner actually asks:
//!
//! | question | what it runs |
//! | --- | --- |
//! | "how many visitors last 30 days" | the summary |
//! | "top pages this week" | the pages breakdown |
//! | "where do my visitors come from" | the referrers breakdown |
//! | "why did traffic drop" | the summary of both periods, and the movers |

use chrono::{Datelike, NaiveDateTime};
use serde::Serialize;

use crate::error::ApiError;
use crate::models::{MetricsSummary, ReportKind};
use crate::storage::{Dimension, ReportQuery, Storage, TimeRange};

/// The metric a question is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Metric {
    /// Total page views.
    Pageviews,
    /// Unique visitors.
    Visitors,
    /// Number of sessions.
    Sessions,
    /// Sessions with a single page view.
    Bounces,
    /// Average session length.
    Duration,
}

impl Metric {
    /// The value of this metric in a summary.
    pub fn value(self, summary: &MetricsSummary) -> i64 {
        match self {
            Metric::Pageviews => summary.pageviews.value,
            Metric::Visitors => summary.visitors.value,
            Metric::Sessions => summary.views.value,
            Metric::Bounces => summary.bounces.value,
            Metric::Duration => summary.visit_duration.value,
        }
    }

    /// The period-over-period change of this metric.
    pub fn change(self, summary: &MetricsSummary) -> f64 {
        match self {
            Metric::Pageviews => summary.pageviews.change,
            Metric::Visitors => summary.visitors.change,
            Metric::Sessions => summary.views.change,
            Metric::Bounces => summary.bounces.change,
            Metric::Duration => summary.visit_duration.change,
        }
    }

    /// The name of the metric, in English.
    pub fn english(self) -> &'static str {
        match self {
            Metric::Pageviews => "page views",
            Metric::Visitors => "visitors",
            Metric::Sessions => "sessions",
            Metric::Bounces => "bounces",
            Metric::Duration => "average visit",
        }
    }

    /// The name of the metric, in English, for **one** of them.
    ///
    /// "1 page views" is the kind of thing that makes a person stop trusting
    /// the rest of the sentence.
    pub fn english_one(self) -> &'static str {
        match self {
            Metric::Pageviews => "page view",
            Metric::Visitors => "visitor",
            Metric::Sessions => "session",
            Metric::Bounces => "bounce",
            Metric::Duration => "average visit",
        }
    }

    /// The name of the metric, in Chinese.
    pub fn chinese(self) -> &'static str {
        match self {
            Metric::Pageviews => "浏览量",
            Metric::Visitors => "访客数",
            Metric::Sessions => "会话数",
            Metric::Bounces => "跳出数",
            Metric::Duration => "平均停留时长",
        }
    }
}

/// What a question is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    /// The window the question is about.
    pub range: TimeRange,
    /// The metric the question asks about.
    pub metric: Metric,
    /// The dimension the question breaks the traffic down by.
    pub dimension: Option<Dimension>,
    /// Whether the question compares with the period before.
    pub compare: bool,
    /// Whether the question asks *why* — which needs the movers, not just the
    /// numbers.
    pub why: bool,
    /// Whether the question is in Chinese.
    pub chinese: bool,
}

impl Default for Intent {
    fn default() -> Self {
        Self {
            range: TimeRange::last_days(7),
            metric: Metric::Pageviews,
            dimension: None,
            compare: false,
            why: false,
            chinese: false,
        }
    }
}

/// One line of a breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    /// The dimension value.
    pub name: String,
    /// How much of it there was.
    pub value: i64,
}

/// A change between two periods.
#[derive(Debug, Clone, Serialize)]
pub struct Mover {
    /// The dimension value.
    pub name: String,
    /// The change in the count.
    pub change: i64,
}

/// What the assistant found out.
///
/// This is the whole answer, in numbers — it is what the wording is made of,
/// and it is what a language model would be given if one were put in front.
#[derive(Debug, Clone, Serialize)]
pub struct Facts {
    /// The question, as it was asked.
    pub question: String,
    /// The window the answer is about.
    pub start: NaiveDateTime,
    /// The end of the window.
    pub end: NaiveDateTime,
    /// The metric the question was about.
    pub metric: Metric,
    /// The summary of the window.
    pub summary: Option<MetricsSummary>,
    /// The breakdown, if the question asked for one.
    pub breakdown: Option<Vec<Row>>,
    /// The dimension the breakdown is over.
    pub dimension: Option<String>,
    /// The biggest movers between this period and the one before it.
    pub movers: Option<Vec<Mover>>,
}

/// The answer: the words, and the numbers they are made of.
#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    /// The answer, in words.
    pub text: String,
    /// The numbers behind it.
    pub facts: Facts,
    /// The report that was run, so the dashboard can show it too.
    pub report: ReportKind,
}

/// Ask a question about a website.
///
/// The question is parsed, the matching report is run, and the answer is
/// written from what came back.
pub fn ask(
    storage: &dyn Storage,
    website: &str,
    question: &str,
    now: NaiveDateTime,
) -> Result<Answer, ApiError> {
    let intent = parse(question, now);
    let query = ReportQuery::new(website, intent.range);

    // The summary is always worth having: it is the answer to most questions,
    // and it is the context of the others. It carries the change against the
    // period before, so the comparison is the same one the dashboard shows.
    let summary = storage.summary_with_change(&query)?;

    let breakdown = match intent.dimension {
        Some(dimension) => Some(
            storage
                .breakdown(&query, dimension)?
                .into_iter()
                .map(|row| Row {
                    name: row.name,
                    value: row.value,
                })
                .collect(),
        ),
        None => None,
    };

    // "Why" needs the period before as well, and the movers between them.
    let movers = if intent.why || intent.compare {
        let dimension = intent.dimension.unwrap_or(Dimension::Url);
        let previous = ReportQuery::new(website, intent.range.previous());
        let current = storage.breakdown(&query, dimension)?;
        let before = storage.breakdown(&previous, dimension)?;
        Some(movers(&current, &before))
    } else {
        None
    };

    let facts = Facts {
        question: question.to_string(),
        start: intent.range.start,
        end: intent.range.end,
        metric: intent.metric,
        summary: Some(summary),
        breakdown,
        dimension: intent.dimension.map(|dimension| dimension_name(dimension).to_string()),
        movers,
    };

    let report = intent.dimension.map(report_for).unwrap_or(ReportKind::Summary);

    Ok(Answer {
        text: compose(&facts, &intent),
        facts,
        report,
    })
}

/// The biggest changes between two periods.
fn movers(
    current: &[crate::models::BreakdownRow],
    before: &[crate::models::BreakdownRow],
) -> Vec<Mover> {
    let before_values: std::collections::HashMap<&str, i64> = before
        .iter()
        .map(|row| (row.name.as_str(), row.value))
        .collect();

    let mut out: Vec<Mover> = current
        .iter()
        .map(|row| Mover {
            name: row.name.clone(),
            change: row.value - before_values.get(row.name.as_str()).copied().unwrap_or(0),
        })
        .collect();

    // The biggest change first, whatever its direction.
    out.sort_by(|a, b| {
        b.change
            .abs()
            .cmp(&a.change.abs())
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(5);
    out
}

/// The report that answers a question about a dimension.
fn report_for(dimension: Dimension) -> ReportKind {
    match dimension {
        Dimension::Url => ReportKind::Pages,
        Dimension::Referrer => ReportKind::Referrers,
        Dimension::Browser => ReportKind::Browsers,
        Dimension::Os => ReportKind::Os,
        Dimension::Device => ReportKind::Devices,
        Dimension::Country => ReportKind::Countries,
        Dimension::Language => ReportKind::Languages,
        Dimension::Screen => ReportKind::Screens,
    }
}

/// The name of a dimension, in English.
fn dimension_name(dimension: Dimension) -> &'static str {
    match dimension {
        Dimension::Url => "pages",
        Dimension::Referrer => "referrers",
        Dimension::Browser => "browsers",
        Dimension::Os => "operating systems",
        Dimension::Device => "devices",
        Dimension::Country => "countries",
        Dimension::Language => "languages",
        Dimension::Screen => "screens",
    }
}

// --------------------------------------------------------------------------------------
// Reading the question
// --------------------------------------------------------------------------------------

/// Read a question.
///
/// The rules are deliberately simple: a window, a metric, a dimension and
/// whether the question compares with the period before. Everything else is
/// ignored — a question the assistant does not understand still gets an answer
/// about the last seven days.
pub fn parse(question: &str, now: NaiveDateTime) -> Intent {
    let text = question.to_lowercase();
    let chinese = is_chinese(question);

    let mut intent = Intent {
        chinese,
        ..Intent::default()
    };

    intent.compare = compare(&text, chinese);
    intent.why = why(&text, chinese);

    // "how many visitors vs last week" asks about **this** week and compares it
    // with last week — so the words after the comparison belong to the other
    // period, and the window is read from what comes before.
    let head = strip_comparison(&text, chinese);

    intent.range = window(&head, chinese, now);
    intent.metric = metric(&head, chinese);
    intent.dimension = dimension(&head, chinese);

    intent
}

/// Cut the question at the comparison, so the window is read from the part that
/// talks about the period being asked about.
///
/// "how many visitors vs last week" → "how many visitors"
/// "最近 30 天的访客数和上周相比" → "最近 30 天的访客数"
fn strip_comparison(text: &str, chinese: bool) -> String {
    let markers: &[&str] = if chinese {
        &["相比", "对比", "比上", "较上", "环比", "同比"]
    } else {
        &[" vs ", " versus ", " compared to ", " compare to ", " against "]
    };

    for marker in markers {
        let Some(position) = text.find(marker) else {
            continue;
        };
        let mut head = &text[..position];
        if chinese {
            // "和上周相比" — the clause starts at the 和, not at the 相比.
            for conjunction in ["和", "与", "跟", "同"] {
                if let Some(at) = head.rfind(conjunction) {
                    if head.len() - at <= 12 {
                        head = &head[..at];
                    }
                }
            }
        }
        return head.trim().to_string();
    }

    text.to_string()
}

/// Is the question in Chinese?
fn is_chinese(question: &str) -> bool {
    question
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// The window the question is about.
fn window(text: &str, chinese: bool, now: NaiveDateTime) -> TimeRange {
    let day = chrono::Duration::days(1);

    if chinese {
        if text.contains("今天") {
            return TimeRange {
                start: now - day,
                end: now,
            };
        }
        if text.contains("昨天") {
            return TimeRange {
                start: now - day * 2,
                end: now - day,
            };
        }
        if text.contains("上周") {
            let end = start_of_week(now);
            return TimeRange {
                start: end - day * 7,
                end,
            };
        }
        if text.contains("本月") || text.contains("这个月") {
            let end = start_of_month(now);
            return TimeRange {
                start: end - day * 30,
                end,
            };
        }
        if text.contains("上个月") || text.contains("上月") {
            let end = start_of_month(now);
            return TimeRange {
                start: end - day * 30,
                end,
            };
        }
        if let Some(days) = days_after(text, &["最近", "过去", "近"]) {
            return TimeRange {
                start: now - chrono::Duration::days(days),
                end: now,
            };
        }
        return TimeRange {
            start: now - day * 7,
            end: now,
        };
    }

    if text.contains("today") {
        return TimeRange {
            start: now - day,
            end: now,
        };
    }
    if text.contains("yesterday") {
        return TimeRange {
            start: now - day * 2,
            end: now - day,
        };
    }
    if text.contains("this week") {
        // This week: from Monday until now.
        return TimeRange {
            start: start_of_week(now),
            end: now,
        };
    }
    if text.contains("last week") {
        // Last week: the seven days before this one.
        let end = start_of_week(now);
        return TimeRange {
            start: end - day * 7,
            end,
        };
    }
    if text.contains("this month") {
        // This month: from the first until now.
        return TimeRange {
            start: start_of_month(now),
            end: now,
        };
    }
    if text.contains("last month") {
        // Last month: the calendar month before this one.
        let end = start_of_month(now);
        let previous = end.date().pred_opt().unwrap_or(end.date());
        let start = previous
            .with_day(1)
            .unwrap_or(previous)
            .and_hms_opt(0, 0, 0)
            .unwrap_or(end);
        return TimeRange { start, end };
    }
    if let Some(days) = days_after(text, &["last", "past", "previous"]) {
        return TimeRange {
            start: now - chrono::Duration::days(days),
            end: now,
        };
    }

    TimeRange {
        start: now - day * 7,
        end: now,
    }
}

/// How many days the question asks about.
///
/// The number has to follow the keyword: "last 30 days", "past 7 days".
fn days_after(text: &str, keywords: &[&str]) -> Option<i64> {
    for keyword in keywords {
        let mut rest = text;
        while let Some(position) = rest.find(keyword) {
            let after = &rest[position + keyword.len()..];
            let digits: String = after
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(days) = digits.parse::<i64>() {
                if days > 0 && days <= 3650 {
                    return Some(days);
                }
            }
            rest = &rest[position + keyword.len()..];
        }
    }
    None
}

/// The start of the week the day is in (Monday).
fn start_of_week(now: NaiveDateTime) -> NaiveDateTime {
    let weekday = now.weekday().num_days_from_monday() as i64;
    (now - chrono::Duration::days(weekday)).date().and_hms_opt(0, 0, 0).unwrap_or(now)
}

/// The start of the month the day is in.
fn start_of_month(now: NaiveDateTime) -> NaiveDateTime {
    let first = now.date().with_day(1).unwrap_or(now.date());
    first.and_hms_opt(0, 0, 0).unwrap_or(now)
}

/// The metric the question is about.
fn metric(text: &str, chinese: bool) -> Metric {
    if chinese {
        if text.contains("跳出") {
            return Metric::Bounces;
        }
        if text.contains("停留") || text.contains("时长") {
            return Metric::Duration;
        }
        if text.contains("访客") {
            return Metric::Visitors;
        }
        if text.contains("会话") {
            return Metric::Sessions;
        }
        if text.contains("浏览") || text.contains("访问量") || text.contains("流量") {
            return Metric::Pageviews;
        }
        return Metric::Pageviews;
    }

    if text.contains("bounce") {
        return Metric::Bounces;
    }
    if text.contains("duration") || text.contains("time on site") || text.contains("how long") {
        return Metric::Duration;
    }
    if text.contains("visitor") {
        return Metric::Visitors;
    }
    if text.contains("session") || text.contains("visit ") || text.contains("visits") {
        return Metric::Sessions;
    }
    Metric::Pageviews
}

/// The dimension the question breaks the traffic down by.
fn dimension(text: &str, chinese: bool) -> Option<Dimension> {
    // "page views" is a **metric**, not a dimension: "how many page views" is
    // not a question about the pages. The metric phrases are taken out before
    // the dimensions are looked for.
    let text = &text
        .replace("page views", " ")
        .replace("pageviews", " ")
        .replace("page view", " ");

    if chinese {
        if text.contains("来源") || text.contains("引荐") || text.contains("referer") || text.contains("referrer") {
            return Some(Dimension::Referrer);
        }
        if text.contains("浏览器") {
            return Some(Dimension::Browser);
        }
        if text.contains("系统") || text.contains("操作系统") {
            return Some(Dimension::Os);
        }
        if text.contains("设备") {
            return Some(Dimension::Device);
        }
        if text.contains("国家") || text.contains("地区") {
            return Some(Dimension::Country);
        }
        if text.contains("语言") {
            return Some(Dimension::Language);
        }
        if text.contains("屏幕") || text.contains("分辨率") {
            return Some(Dimension::Screen);
        }
        if text.contains("事件") {
            return Some(Dimension::Url);
        }
        if text.contains("页面") || text.contains("网页") || text.contains("路径") {
            return Some(Dimension::Url);
        }
        return None;
    }

    if text.contains("referrer") || text.contains("referer") || text.contains("traffic source")
        || text.contains("where do") || text.contains("where from") || text.contains("source")
    {
        return Some(Dimension::Referrer);
    }
    if text.contains("browser") {
        return Some(Dimension::Browser);
    }
    if text.contains("operating system") || text.contains(" os ") || text.contains("os?") {
        return Some(Dimension::Os);
    }
    if text.contains("device") {
        return Some(Dimension::Device);
    }
    if text.contains("country") || text.contains("countries") || text.contains("region") {
        return Some(Dimension::Country);
    }
    if text.contains("language") {
        return Some(Dimension::Language);
    }
    if text.contains("screen") || text.contains("resolution") {
        return Some(Dimension::Screen);
    }
    if text.contains("event") {
        return Some(Dimension::Url);
    }
    if text.contains("page") || text.contains("url") || text.contains("path") || text.contains("blog")
    {
        return Some(Dimension::Url);
    }
    None
}

/// Does the question compare with the period before?
fn compare(text: &str, chinese: bool) -> bool {
    if chinese {
        return ["变化", "对比", "相比", "比上", "环比", "同比", "下降", "上涨", "增长", "跌", "趋势"]
            .iter()
            .any(|word| text.contains(word));
    }
    [" vs ", "versus", "compare", "compared", "than before", "change", "trend", "drop", "dropped", "rise", "grew"]
        .iter()
        .any(|word| text.contains(word))
}

/// Does the question ask *why*?
fn why(text: &str, chinese: bool) -> bool {
    if chinese {
        return ["为什么", "原因", "怎么回事", "咋回事"]
            .iter()
            .any(|word| text.contains(word));
    }
    ["why", "reason", "cause"]
        .iter()
        .any(|word| text.contains(word))
}

// --------------------------------------------------------------------------------------
// Writing the answer
// --------------------------------------------------------------------------------------

/// Write the answer from the facts.
///
/// The wording is the only place where the assistant sounds like a person; every
/// number in it comes from [`Facts`].
pub fn compose(facts: &Facts, intent: &Intent) -> String {
    let chinese = intent.chinese;
    let summary = facts.summary.as_ref();

    let mut sentences: Vec<String> = Vec::new();

    // The headline: the metric that was asked about.
    if let Some(summary) = summary {
        let metric = facts.metric;
        let value = metric.value(summary);
        sentences.push(if chinese {
            format!(
                "最近这段时间（{}）：{} {}。",
                period(facts, chinese),
                number(value),
                metric.chinese()
            )
        } else {
            format!(
                "Over the last {}: {} {}.",
                period(facts, chinese),
                number(value),
                // One is one.
                if value == 1 {
                    metric.english_one()
                } else {
                    metric.english()
                }
            )
        });

        // The rest of the summary, when the question was not about one metric.
        if metric == Metric::Pageviews && intent.dimension.is_none() {
            sentences.push(if chinese {
                format!(
                    "{} 访客，{} 次会话，{} 次跳出，平均停留 {}。",
                    number(summary.visitors.value),
                    number(summary.views.value),
                    number(summary.bounces.value),
                    duration(summary.visit_duration.value)
                )
            } else {
                format!(
                    "{} visitors, {} sessions, {} bounces, {} average visit.",
                    number(summary.visitors.value),
                    number(summary.views.value),
                    number(summary.bounces.value),
                    duration(summary.visit_duration.value)
                )
            });
        }
    }

    // The change against the period before.
    if intent.compare {
        if let Some(summary) = summary {
            let change = facts.metric.change(summary);
            sentences.push(if chinese {
                format!("和上一个{}相比，{} {} {:+.1}%。", period(facts, chinese), facts.metric.chinese(), if change >= 0.0 { "上涨了" } else { "下降了" }, change.abs())
            } else {
                format!(
                    "That is {:+.1}% against the previous {}.",
                    change,
                    period(facts, chinese)
                )
            });
        }
    }

    // The breakdown.
    if let Some(rows) = facts.breakdown.as_ref() {
        let dimension = facts.dimension.as_deref().unwrap_or("pages");
        if !rows.is_empty() {
            let list: Vec<String> = rows
                .iter()
                .take(5)
                .map(|row| format!("{} ({})", row.name, number(row.value)))
                .collect();
            sentences.push(if chinese {
                format!("最多的{}：{}。", dimension, list.join("、"))
            } else {
                format!("The top {}: {}.", dimension, list.join(", "))
            });
        }
    }

    // The movers: the answer to "why".
    if let Some(movers) = facts.movers.as_ref() {
        let changed: Vec<&Mover> = movers.iter().filter(|m| m.change != 0).collect();
        if !changed.is_empty() {
            let list: Vec<String> = changed
                .iter()
                .take(3)
                .map(|mover| format!("{} ({:+})", mover.name, number(mover.change)))
                .collect();
            sentences.push(if chinese {
                format!("变化最大的：{}。", list.join("、"))
            } else {
                format!("The biggest movers: {}.", list.join(", "))
            });
        }
    }

    if sentences.is_empty() {
        return if chinese {
            "这段时间没有数据。".to_string()
        } else {
            "There is no data for that period.".to_string()
        };
    }

    sentences.join(" ")
}

/// The period, in words.
fn period(facts: &Facts, chinese: bool) -> String {
    let days = (facts.end - facts.start).num_days().max(1);
    if chinese {
        format!("{days} 天")
    } else {
        format!("{days} days")
    }
}

/// A number with the thousand separators a person expects.
fn number(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(character);
    }
    if negative {
        format!("-{out}")
    } else {
        out
    }
}

/// A duration in seconds, in words.
fn duration(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let rest = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m {rest}s");
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}
