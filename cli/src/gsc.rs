//! The Google Search Console export.
//!
//! The export is a CSV file with a header row:
//!
//! ```text
//! Date,Query,Page,Clicks,Impressions,CTR,Position
//! 2026-03-01,sumami analytics,https://example.com/blog,42,3100,0.0135,3.2
//! ```
//!
//! The columns are looked up **by name**, so an export with extra columns (or with
//! `Country` instead of `Query`) still reads. The rates are read the way Google
//! writes them: `0.0135` or `1.35%` are both 1.35%.

use anyhow::{anyhow, Result};
use sumami_service::instance::GscRow;

/// Parse a Search Console export.
pub fn parse(text: &str) -> Result<Vec<GscRow>> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("the export is empty"))?;

    let columns: Vec<String> = split_csv(header)
        .into_iter()
        .map(|name| name.trim().to_lowercase())
        .collect();

    let index = |name: &str| columns.iter().position(|column| column == name);
    let date_at = index("date");
    let query_at = index("query");
    let page_at = index("page");
    let clicks_at = index("clicks");
    let impressions_at = index("impressions");
    let ctr_at = index("ctr");
    let position_at = index("position");

    if date_at.is_none() {
        return Err(anyhow!(
            "the export has no `Date` column — is this a Search Console export?"
        ));
    }

    let mut rows = Vec::new();
    for (number, line) in lines.enumerate() {
        let fields = split_csv(line);
        let field = |at: Option<usize>| -> Option<String> {
            at.and_then(|index| fields.get(index))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };

        let Some(date) = field(date_at) else {
            // A row without a date is not a row.
            continue;
        };

        rows.push(GscRow {
            date,
            query: field(query_at),
            page: field(page_at),
            clicks: field(clicks_at)
                .and_then(|value| value.replace(',', "").parse::<i64>().ok())
                .unwrap_or(0),
            impressions: field(impressions_at)
                .and_then(|value| value.replace(',', "").parse::<i64>().ok())
                .unwrap_or(0),
            ctr: field(ctr_at).and_then(|value| parse_rate(&value)),
            position: field(position_at).and_then(|value| value.parse::<f64>().ok()),
        });

        if number > 1_000_000 {
            return Err(anyhow!("the export has more than a million rows"));
        }
    }

    Ok(rows)
}

/// Split one CSV line.
///
/// The export quotes the fields that contain a comma — a query like
/// `rust, duckdb` — so the quotes have to be honoured.
fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;

    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted => {
                // A doubled quote is one quote.
                if characters.peek() == Some(&'"') {
                    characters.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut field));
            }
            other => field.push(other),
        }
    }
    fields.push(field);
    fields
}

/// A rate, written either as a fraction (`0.0135`) or as a percentage (`1.35%`).
fn parse_rate(value: &str) -> Option<f64> {
    let value = value.trim();
    if let Some(number) = value.strip_suffix('%') {
        return number.trim().parse::<f64>().ok().map(|rate| rate / 100.0);
    }
    value.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_search_console_export() {
        let rows = parse(
            "Date,Query,Page,Clicks,Impressions,CTR,Position\n\
             2026-03-01,sumami analytics,https://example.com/blog,42,3100,1.35%,3.2\n\
             2026-03-02,,https://example.com/,7,900,0.78%,8.1\n",
        )
        .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date, "2026-03-01");
        assert_eq!(rows[0].query.as_deref(), Some("sumami analytics"));
        assert_eq!(rows[0].clicks, 42);
        assert_eq!(rows[0].impressions, 3100);
        // A percentage is a rate.
        assert!((rows[0].ctr.unwrap() - 0.0135).abs() < 0.0001);
        assert!((rows[0].position.unwrap() - 3.2).abs() < 0.001);
        // An empty cell is missing, not empty.
        assert_eq!(rows[1].query, None);
    }

    #[test]
    fn a_query_with_a_comma_is_one_field() {
        let rows = parse(
            "Date,Query,Page,Clicks,Impressions,CTR,Position\n\
             2026-03-01,\"rust, duckdb\",https://example.com/,1,10,10%,1\n",
        )
        .unwrap();
        assert_eq!(rows[0].query.as_deref(), Some("rust, duckdb"));
    }

    #[test]
    fn an_export_without_a_date_is_refused() {
        assert!(parse("Query,Clicks\nsumami,1\n").is_err());
    }
}
