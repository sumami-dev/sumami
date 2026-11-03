//! Human-readable output.

use serde_json::Value;

/// Print a table with aligned columns.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let separator = {
        let mut line = String::new();
        for (index, width) in widths.iter().enumerate() {
            if index > 0 {
                line.push_str("  ");
            }
            line.push_str(&"-".repeat(*width));
        }
        line
    };

    println!("{}", join_row(headers.iter().map(|h| h.to_string()), &widths));
    println!("{separator}");
    for row in rows {
        println!("{}", join_row(row.iter().cloned(), &widths));
    }
}

/// Join one row with padded columns.
fn join_row(cells: impl Iterator<Item = String>, widths: &[usize]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        let width = widths.get(index).copied().unwrap_or(0);
        let length = cell.chars().count();
        line.push_str(&cell);
        for _ in length..width {
            line.push(' ');
        }
    }
    line.trim_end().to_string()
}

/// Format a JSON value as a cell.
pub fn cell(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// Format a number for the metric tables.
pub fn number(value: i64) -> String {
    let text = value.abs().to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index > 0 && (text.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 {
        format!("-{out}")
    } else {
        out
    }
}
