//! Charts drawn with block characters, for `spares card statistics`.

use std::fmt::Write;

use colored::Color;
use colored::Colorize;

/// Eighths of a cell, from empty to full.
const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// A column chart: one column per entry of `columns`, each a stack of values from the bottom up
/// coloured by `colors`, scaled so the tallest stack is `height` rows. Columns are merged, adding
/// up their values, until there are at most `width` of them, and each is `column_width`
/// characters wide. `labels` go under the first and last columns, e.g. their dates.
///
/// A stack's top is drawn to an eighth of a row. Within a stack, each cell takes the colour of the
/// value filling most of it, so with colour off only the totals show.
pub(crate) fn column_chart(
    columns: &[Vec<f64>],
    colors: &[Color],
    height: usize,
    width: usize,
    column_width: usize,
    labels: (&str, &str),
) -> String {
    let columns = merge(columns, (width / column_width.max(1)).max(1));
    let totals = columns
        .iter()
        .map(|stack| stack.iter().sum::<f64>())
        .collect::<Vec<_>>();
    let max = totals.iter().copied().fold(0.0, f64::max);
    let max_label = format_value(max);
    let axis_width = max_label.len().max(1);
    let mut out = String::new();
    for row in (0..height).rev() {
        let axis = if row == height - 1 {
            format!("{max_label:>axis_width$} ┤")
        } else {
            format!("{:>axis_width$} │", "")
        };
        let mut line = axis;
        for stack in &columns {
            let cell = cell(stack, max, height, row, colors);
            for _ in 0..column_width {
                line.push_str(&cell);
            }
        }
        writeln!(out, "{}", line.trim_end()).unwrap();
    }
    let chart_width = columns.len() * column_width;
    writeln!(out, "{:>axis_width$} └{}", 0, "─".repeat(chart_width)).unwrap();
    let (first, last) = labels;
    let gap = chart_width.saturating_sub(first.len() + last.len()).max(1);
    let labels = if last.is_empty() {
        first.to_string()
    } else {
        format!("{first}{}{last}", " ".repeat(gap))
    };
    writeln!(out, "{}{labels}", " ".repeat(axis_width + 2)).unwrap();
    out
}

/// The cell of `row` in a column with `stack`, as a block character in the colour of the value
/// filling most of it.
fn cell(stack: &[f64], max: f64, height: usize, row: usize, colors: &[Color]) -> String {
    if max <= 0.0 {
        return " ".to_string();
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "heights are a few dozen eighths"
    )]
    let eighths = |value: f64| (value / max * (height * 8) as f64).round() as usize;
    let top = eighths(stack.iter().sum());
    let filled = top.saturating_sub(row * 8).min(8);
    // Each value's top, in eighths
    let mut tops = Vec::with_capacity(stack.len());
    let mut sum = 0.0;
    for value in stack {
        sum += value;
        tops.push(eighths(sum));
    }
    if filled == 0 {
        // A column with anything at all shows at least its lowest eighth
        return if row == 0 && sum > 0.0 {
            paint(EIGHTHS[1], color_at(&tops, 0, colors))
        } else {
            " ".to_string()
        };
    }
    let middle = row * 8 + filled / 2;
    paint(EIGHTHS[filled], color_at(&tops, middle, colors))
}

fn color_at(tops: &[usize], eighth: usize, colors: &[Color]) -> Option<Color> {
    let index = tops
        .iter()
        .position(|top| *top > eighth)
        .unwrap_or(tops.len().saturating_sub(1));
    colors.get(index).copied()
}

fn paint(glyph: char, color: Option<Color>) -> String {
    match color {
        Some(color) => glyph.to_string().color(color).to_string(),
        None => glyph.to_string(),
    }
}

/// `columns` merged into at most `max` columns of the same number of entries (the last one
/// possibly fewer), each adding up its entries' stacks.
fn merge(columns: &[Vec<f64>], max: usize) -> Vec<Vec<f64>> {
    if columns.len() <= max {
        return columns.to_vec();
    }
    let per_column = columns.len().div_ceil(max);
    columns
        .chunks(per_column)
        .map(|chunk| {
            let depth = chunk.iter().map(Vec::len).max().unwrap_or(0);
            (0..depth)
                .map(|i| chunk.iter().filter_map(|stack| stack.get(i)).sum())
                .collect()
        })
        .collect()
}

/// A value for an axis: whole when it is at least 10, else to one decimal.
pub(crate) fn format_value(value: f64) -> String {
    if value >= 10.0 || value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// `█ learning  █ review`, each block in its series' colour.
pub(crate) fn legend(series: &[(&str, Color)]) -> String {
    series
        .iter()
        .map(|(name, color)| format!("{} {name}", "█".color(*color)))
        .collect::<Vec<_>>()
        .join("  ")
}

/// A horizontal bar `width` characters at its longest, for `value` out of `max`. At least one
/// eighth for any value above 0.
pub(crate) fn bar(value: f64, max: f64, width: usize) -> String {
    if max <= 0.0 || value <= 0.0 {
        return String::new();
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "bars are a few hundred eighths"
    )]
    let eighths = ((value / max * (width * 8) as f64).round() as usize).max(1);
    let mut bar = "█".repeat(eighths / 8);
    let rest = eighths % 8;
    if rest > 0 {
        // The left eighths, from one to seven
        bar.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][rest - 1]);
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() {
        colored::control::set_override(false);
    }

    #[test]
    fn columns_scale_to_the_tallest() {
        plain();
        let columns = [vec![4.0], vec![2.0], vec![0.0], vec![1.0]];
        let out = column_chart(&columns, &[Color::Green], 2, 80, 1, ("a", "b"));
        assert_eq!(out, "4 ┤█\n  │██ ▄\n0 └────\n   a  b\n");
    }

    #[test]
    fn a_small_value_still_shows() {
        plain();
        let columns = [vec![100.0], vec![1.0]];
        let out = column_chart(&columns, &[Color::Green], 2, 80, 2, ("", ""));
        assert_eq!(out.lines().nth(1).unwrap(), "    │██▁▁");
    }

    #[test]
    fn columns_merge_to_fit() {
        let columns = (1..=10).map(|i| vec![f64::from(i)]).collect::<Vec<_>>();
        assert_eq!(
            merge(&columns, 4),
            [vec![6.0], vec![15.0], vec![24.0], vec![10.0]]
        );
        assert_eq!(merge(&columns, 10).len(), 10);
    }

    #[test]
    fn stacks_add_up() {
        let columns = [vec![1.0, 2.0], vec![3.0]];
        assert_eq!(merge(&columns, 1), [vec![4.0, 2.0]]);
    }

    #[test]
    fn bars_end_in_eighths() {
        assert_eq!(bar(10.0, 10.0, 4), "████");
        assert_eq!(bar(5.0, 10.0, 3), "█▌");
        assert_eq!(bar(0.01, 10.0, 4), "▏");
        assert_eq!(bar(0.0, 10.0, 4), "");
    }

    #[test]
    fn values() {
        assert_eq!(format_value(0.0), "0");
        assert_eq!(format_value(2.5), "2.5");
        assert_eq!(format_value(12.4), "12");
    }
}
