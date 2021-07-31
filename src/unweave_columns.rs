// Copyright 2022 Alexandros Frantzis
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more
// details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{UnweaveOptionsColumns, UnweaveTwoPass, UnweaveWidth, UnweaveTab};
use crate::util::{TagFinder, FileLines, trim_newline, SliceFullLines, FileContents,
                  ascii_grapheme_count, str_grapheme_count, grapheme_count_tab_expanded,
                  for_each_grapheme, Grapheme};

use ahash::AHashMap;
use anyhow::{Result, Context};

use std::io::{Write, BufWriter, self};
use std::fs::File;
use std::num::NonZeroU32;

/// Helper that handles printing of lines at specific columns.
struct ColumnPrinter {
    bufwriter: Box<dyn Write>,
    sep: String,
    tab: UnweaveTab,
    column_widths: Vec<u32>,
    column_prefixes: Vec<String>,
    column_suffixes: Vec<String>,
}

impl ColumnPrinter {
    /// Create a new ColumnPrinter.
    fn new(opts: &UnweaveOptionsColumns) -> Result<Self> {
        Ok(
            ColumnPrinter {
                bufwriter: match &opts.output {
                    Some(o) => Box::new(
                        BufWriter::new(
                            File::create(o).with_context(
                                || format!("Failed to create output file {}", o.display())
                            )?
                        )
                    ),
                    None => Box::new(BufWriter::new(io::stdout())),
                },
                sep: match &opts.column_separator {
                    Some(s) => s.clone(),
                    None => "".to_string(),
                },
                tab: opts.tab,
                column_widths: Vec::new(),
                column_prefixes: Vec::new(),
                column_suffixes: Vec::new(),
            }
        )
    }

    /// Sets the number of columns and their widths to print with.
    fn set_column_widths(&mut self, column_widths: &[u32]) {
        self.column_widths = column_widths.to_vec();
        self.column_prefixes.clear();
        self.column_suffixes.clear();

        for col in 0..column_widths.len() {
            let mut prefix = String::new();
            let mut suffix = String::new();

            for w in &column_widths[..col] {
                prefix.push_str(&(" ".repeat(*w as usize) + &self.sep));
            }

            for w in &column_widths[col+1..] {
                suffix.push_str(&(self.sep.clone() + &" ".repeat(*w as usize)));
            }
            suffix.truncate(suffix.trim_end().len());
            suffix.push('\n');

            self.column_prefixes.push(prefix);
            self.column_suffixes.push(suffix);
        }
    }

    /// Print data in a column, assuming that the data can fit without
    /// wrapping.
    fn print_in_column_unwrapped(&mut self, chunk: &[u8], col: u32,
                                 grapheme_count: Option<NonZeroU32>) -> Result<()> {
        let col = col as usize;
        let column_width = self.column_widths[col];

        self.bufwriter.write(self.column_prefixes[col].as_bytes())?;
        self.bufwriter.write(chunk)?;
        // Avoid trailing whitespace
        if !self.column_suffixes[col].trim_end().is_empty() {
            let grapheme_count = match grapheme_count {
                Some(g) => g.get(),
                _ => grapheme_count_tab_expanded(chunk, self.tab, None)
            };

            // Fill in to reach required width
            let mut remaining = if grapheme_count < column_width {
                column_width - grapheme_count
            } else {
                0
            };

            while remaining > 0 {
                if remaining >= 8 {
                    self.bufwriter.write(b"        ")?;
                    remaining -= 8;
                } else if remaining >= 4 {
                    self.bufwriter.write(b"    ")?;
                    remaining -= 4;
                } else if remaining >= 2 {
                    self.bufwriter.write(b"  ")?;
                    remaining -= 2;
                } else if remaining >= 1 {
                    self.bufwriter.write(b" ")?;
                    remaining -= 1;
                }
            }
        }

        self.bufwriter.write(self.column_suffixes[col].as_bytes())?;

        Ok(())
    }

    /// Print data in a column, will wrap if needed.
    fn print_in_column(&mut self, line: &[u8], col: u32,
                       mut grapheme_count: Option<NonZeroU32>) -> Result<()> {
        let column_width = self.column_widths[col as usize];
        let mut chunk_graphemes = 0;
        let mut chunk_start = 0;
        let mut chunk_end = 0;
        let mut untabbed_line = Vec::new();

        let line = if self.tab.is_expand() && line.contains(&b'\t') {
            grapheme_count = NonZeroU32::new(
                grapheme_count_tab_expanded(line, self.tab, Some(&mut untabbed_line))
            );
            &untabbed_line
        } else {
            line
        };

        let max_grapheme_count = match grapheme_count {
            Some(g) => g.get(),
            _ => line.len() as u32,
        };

        // Fast path for when we are sure the line doesn't need to be split.
        if max_grapheme_count <= column_width {
            return self.print_in_column_unwrapped(line, col, grapheme_count);
        }

        for_each_grapheme(line,
            |g| {
                match g {
                    Grapheme::Unicode(s) => {
                        chunk_graphemes += str_grapheme_count(s);
                        chunk_end += s.len();
                    },
                    Grapheme::Ascii(b) => {
                        chunk_graphemes += ascii_grapheme_count(b);
                        chunk_end += 1;
                    }
                };

                // If this is not the end of the column chunk, continue.
                if chunk_end < line.len() && chunk_graphemes < column_width {
                    return Ok(());
                }

                let chunk = &line[chunk_start..chunk_end];

                self.print_in_column_unwrapped(chunk, col,
                                               NonZeroU32::new(chunk_graphemes))?;

                chunk_start = chunk_end;
                chunk_graphemes = 0;
                Ok(())
            }
        )?;

        Ok(())
    }
}

/// Tracks the number of columns and their widths.
struct ColumnTracker<'a> {
    opts: &'a UnweaveOptionsColumns,
    tag_finder: TagFinder,
    column_for_tag: AHashMap<Vec<u8>, u32>,
    column_widths: Vec<u32>,
}

impl<'a> ColumnTracker<'a> {
    /// Creates a new ColumnTracker.
    fn new(opts: &'a UnweaveOptionsColumns) -> Result<Self> {
        Ok(
            Self {
                opts,
                tag_finder: TagFinder::new(&opts.pattern)?,
                column_for_tag: AHashMap::new(),
                column_widths: Vec::new(),
            }
        )
    }

    /// Convenience function to process a line, without updating any ColumnPrinter
    /// instance.
    fn process_line(&mut self, line: &[u8]) -> Option<(u32,Option<NonZeroU32>)> {
        self.process_line_with_column_printer(line, None)
    }

    /// Process a line, updating tracking information about the required columns
    /// and their width.
    ///
    /// Optionally, if a ColumnPrinter instance is provided, the instance
    /// updated with any new column information.
    ///
    /// Returns the column the line belongs in, or None, if the line should be
    /// ignored.
    fn process_line_with_column_printer(&mut self, line: &[u8],
                                        lp: Option<&mut ColumnPrinter>) -> Option<(u32,Option<NonZeroU32>)> {
        let tag = match self.tag_finder.find_in(&line) {
            Some(tag_range) => &line[tag_range],
            None => return None,
        };

        let grapheme_count = match self.opts.width { 
            UnweaveWidth::Undefined => NonZeroU32::new(
                grapheme_count_tab_expanded(line, self.opts.tab, None)
            ),
            _ => None
        };

        let column_width = match self.opts.width { 
            UnweaveWidth::Undefined => grapheme_count.map_or(0, |g| g.get()),
            UnweaveWidth::Column(w) => w,
            _ => 0
        };

        let column = match self.column_for_tag.get(tag) {
            Some(c) => {
                self.column_widths[*c as usize] =
                    std::cmp::max(self.column_widths[*c as usize], column_width);
                *c
            }
            None => {
                let c = self.column_for_tag.len() as u32;
                self.column_for_tag.insert(tag.to_vec(), c);
                self.column_widths.push(column_width);
                if let Some(lp) = lp {
                    lp.set_column_widths(&self.column_widths);
                }
                c
            }
        };

        Some((column, grapheme_count))
    }

    /// Returns the final column widths, in case they need to be adjusted
    /// due to options.
    fn final_column_widths(&mut self) -> &[u32] {
        match self.opts.width {
            UnweaveWidth::Line(w) => {
                let ncolumns = self.column_widths.len() as u32;
                for cw in self.column_widths.iter_mut() { *cw = w / ncolumns; }
            },
            _ => {},
        };

        &self.column_widths
    }
}

/// Perform the unweave operation into columns using a single pass of the data.
///
/// Note that single pass is only possible in limited circumstances (see
/// unweave_into_columns where the decision is made).
fn unweave_into_columns_single_pass(opts: &UnweaveOptionsColumns) -> Result<()> {
    let mut column_printer = ColumnPrinter::new(&opts)?;
    let mut column_tracker = ColumnTracker::new(&opts)?;

    for input in &opts.inputs {
        let mut file_lines = FileLines::new(input, opts.mmap)?;
        while let Some(line) = file_lines.next() {
            match column_tracker.process_line_with_column_printer(line, Some(&mut column_printer)) {
                Some((column, grapheme_count)) => column_printer.print_in_column(line, column, grapheme_count)?,
                None => continue,
            }
        }
    }

    Ok(())
}

/// Perform the unweave operation into columns using two passes, using cached
/// data from the first pass (including loaded file contents), to speed up
/// the second pass.
fn unweave_into_columns_two_pass_cached(opts: &UnweaveOptionsColumns) -> Result<()> {
    let mut column_tracker = ColumnTracker::new(&opts)?;

    let mut file_contents_vec = Vec::new();
    let mut lines_vec = Vec::new();

    // First pass gets file contents and lines/column info
    for input in &opts.inputs {
        let file_contents = FileContents::new(input, opts.mmap)?;
        let mut lines = Vec::new();
        let mut cur = 0;

        for line in SliceFullLines::new(file_contents.contents()) {
            let trimmed_line = trim_newline(line);

            match column_tracker.process_line(trimmed_line) {
                Some((column, grapheme_count)) => lines.push((cur..cur+trimmed_line.len(), column, grapheme_count)),
                None => {},
            }

            cur += line.len();
        }

        file_contents_vec.push(file_contents);
        lines_vec.push(lines);
    }

    let mut column_printer = ColumnPrinter::new(&opts)?;
    column_printer.set_column_widths(column_tracker.final_column_widths());

    // Second pass, which now has all the line and column information, prints
    // out the data.
    for (file_contents, lines) in file_contents_vec.iter().zip(lines_vec.iter()) {
        let contents = file_contents.contents();
        for (line_range, col, grapheme_count) in lines {
            column_printer.print_in_column(&contents[line_range.clone()], *col, *grapheme_count)?;
        }
    }

    Ok(())
}

/// Perform the unweave operation into columns using two passes, maintaining
/// only very limited information between passes, requiring a reread
/// of the data during the second pass.
fn unweave_into_columns_two_pass_reread(opts: &UnweaveOptionsColumns) -> Result<()> {
    let mut column_tracker = ColumnTracker::new(&opts)?;

    // First pass populates column info
    for input in &opts.inputs {
        let mut file_lines = FileLines::new(input, opts.mmap)?;
        while let Some(line) = file_lines.next() {
            column_tracker.process_line(line);
        }
    }

    let mut column_printer = ColumnPrinter::new(&opts)?;
    column_printer.set_column_widths(column_tracker.final_column_widths());

    // Second pass prints the columns
    for input in &opts.inputs {
        let mut file_lines = FileLines::new(input, opts.mmap)?;
        while let Some(line) = file_lines.next() {
            match column_tracker.process_line(line) {
                Some((column, grapheme_count)) =>
                    column_printer.print_in_column(&line, column, grapheme_count)?,
                None => continue,
            }
        }
    }

    Ok(())
}

/// Perform the unweave operation into multiple columns, one column per matched stream.
pub(crate) fn unweave_into_columns(opts: &UnweaveOptionsColumns) -> Result<()> {
    if opts.column_separator.is_none() && opts.width.is_column() {
        return unweave_into_columns_single_pass(&opts);
    }

    match opts.two_pass {
        UnweaveTwoPass::Cached => unweave_into_columns_two_pass_cached(&opts),
        UnweaveTwoPass::Reread => unweave_into_columns_two_pass_reread(&opts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempdir::TempDir;
    use std::fs::{self};
    use crate::{UnweaveMmap, UnweaveTwoPass};

    struct TestParams {
        mmap: UnweaveMmap,
        two_pass: UnweaveTwoPass,
    }

    const TEST_PARAMS: &[TestParams] = &[
        TestParams{ mmap: UnweaveMmap::Allow, two_pass: UnweaveTwoPass::Cached },
        TestParams{ mmap: UnweaveMmap::Allow, two_pass: UnweaveTwoPass::Reread },
        TestParams{ mmap: UnweaveMmap::Disallow, two_pass: UnweaveTwoPass::Cached },
        TestParams{ mmap: UnweaveMmap::Disallow, two_pass: UnweaveTwoPass::Reread },
    ];

    fn unweave_columns_simple_pattern_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"A:1\nB:1\nA:2\nZ:1\nC:1\nB:2\nC:2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: None,
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("A:1\n",
                        "     B:1\n",
                        "A:2\n",
                        "          C:1\n",
                        "     B:2\n",
                        "          C:2\n").as_bytes());
    }

    #[test]
    fn unweave_columns_simple_pattern() {
        for test_params in TEST_PARAMS {
            unweave_columns_simple_pattern_with_params(test_params);
        }
    }

    fn unweave_columns_separator_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"A:1\nB:1\nA:2\nZ:1\nC:1\nB:2\nC:2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("A:1  |     |\n",
                        "     |B:1  |\n",
                        "A:2  |     |\n",
                        "     |     |C:1\n",
                        "     |B:2  |\n",
                        "     |     |C:2\n").as_bytes());
    }

    #[test]
    fn unweave_columns_separator() {
        for test_params in TEST_PARAMS {
            unweave_columns_separator_with_params(test_params);
        }
    }

    fn unweave_columns_auto_width_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"A:11\nB:1111\nA:2\nZ:1\nC:1\nB:2\nC:222").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Undefined,
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("A:11|      |\n",
                        "    |B:1111|\n",
                        "A:2 |      |\n",
                        "    |      |C:1\n",
                        "    |B:2   |\n",
                        "    |      |C:222\n").as_bytes());
    }

    #[test]
    fn unweave_columns_auto_width() {
        for test_params in TEST_PARAMS {
            unweave_columns_auto_width_with_params(test_params);
        }
    }

    fn unweave_columns_line_width_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"A:11\nB:111\nA:2\nZ:1\nC:1\nB:2\nC:222").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Line(15),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("A:11 |     |\n",
                        "     |B:111|\n",
                        "A:2  |     |\n",
                        "     |     |C:1\n",
                        "     |B:2  |\n",
                        "     |     |C:222\n").as_bytes());
    }

    #[test]
    fn unweave_columns_line_width() {
        for test_params in TEST_PARAMS {
            unweave_columns_line_width_with_params(test_params);
        }
    }

    fn unweave_columns_complex_regex_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"1ACx\n2BAy\n3AC\nZAC\n4CBz\n5BAz\n6CCy").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: r"[1-6](A|B|C)(?:A|B|C)".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Undefined,
            column_separator: None,
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("1ACx\n",
                        "    2BAy\n",
                        "3AC\n",
                        "        4CBz\n",
                        "    5BAz\n",
                        "        6CCy\n").as_bytes());
    }

    #[test]
    fn unweave_columns_complex_regex() {
        for test_params in TEST_PARAMS {
            unweave_columns_complex_regex_with_params(test_params);
        }
    }

    fn unweave_columns_multiple_unicode_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1"), tmpdir.path().join("input2")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "άλφα1\nβήτα1\nδέλτα1\nβήτα2\nγάμμα1\n".as_bytes()).unwrap();
        fs::write(&inputs[1], "γάμμα2\nάλφα2\n".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "άλφα|βήτα|γάμμα".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Undefined,
            column_separator: None,
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("άλφα1\n",
                        "     βήτα1\n",
                        "     βήτα2\n",
                        "          γάμμα1\n",
                        "          γάμμα2\n",
                        "άλφα2\n").as_bytes());
    }

    #[test]
    fn unweave_columns_multiple_unicode() {
        for test_params in TEST_PARAMS {
            unweave_columns_multiple_unicode_with_params(test_params);
        }
    }

    fn unweave_columns_wrapping_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "άλφα-1\nβήτα1\nδέλτα1\nβήτα-22\nγάμμα1\n".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "άλφα|βήτα|γάμμα".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: None,
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("άλφα-\n",
                        "1\n",
                        "     βήτα1\n",
                        "     βήτα-\n",
                        "     22\n",
                        "          γάμμα\n",
                        "          1\n").as_bytes());
    }

    #[test]
    fn unweave_columns_wrapping() {
        for test_params in TEST_PARAMS {
            unweave_columns_wrapping_with_params(test_params);
        }
    }

    fn unweave_columns_wrapping_with_separator_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "άλφα-1\nβήτα1\nδέλτα1\nβήτα-1234567\nγάμμα1\n".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "άλφα|βήτα|γάμμα".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("##".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("άλφα-##     ##\n",
                        "1    ##     ##\n",
                        "     ##βήτα1##\n",
                        "     ##βήτα-##\n",
                        "     ##12345##\n",
                        "     ##67   ##\n",
                        "     ##     ##γάμμα\n",
                        "     ##     ##1\n").as_bytes());
    }

    #[test]
    fn unweave_columns_wrapping_with_separator() {
        for test_params in TEST_PARAMS {
            unweave_columns_wrapping_with_separator_with_params(test_params);
        }
    }

    fn unweave_columns_unicode_fill_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "α:Α\nβ:ΒΒ\nγ:ΓΓΓ".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "α|β|γ".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                concat!("α:Α  |     |\n",
                        "     |β:ΒΒ |\n",
                        "     |     |γ:ΓΓΓ\n").as_bytes());
    }

    #[test]
    fn unweave_columns_unicode_fill() {
        for test_params in TEST_PARAMS {
            unweave_columns_unicode_fill_with_params(test_params);
        }
    }

    fn unweave_columns_invalid_unicode_fill_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"\xce\xb1\xce\x79\n\xce\xb2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "α|β".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                b"\xce\xb1\xce\x79  |\n     |\xce\xb2\n");
    }

    #[test]
    fn unweave_columns_invalid_unicode_fill() {
        for test_params in TEST_PARAMS {
            unweave_columns_invalid_unicode_fill_with_params(test_params);
        }
    }

    fn unweave_columns_invalid_unicode_wrap_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"\xce\xb1\xce\x79\n\xce\xb2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "α|β".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(1),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                b"\xce\xb1|\n\xce|\n\x79|\n |\xce\xb2\n");
    }

    #[test]
    fn unweave_columns_invalid_unicode_wrap() {
        for test_params in TEST_PARAMS {
            unweave_columns_invalid_unicode_wrap_with_params(test_params);
        }
    }

    fn unweave_columns_invalid_unicode_not_printable_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"\xce\xb1\xce\x13\n\xce\xb2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "α|β".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                b"\xce\xb1\xce\x13   |\n     |\xce\xb2\n");
    }

    #[test]
    fn unweave_columns_invalid_unicode_not_printable_wrap() {
        for test_params in TEST_PARAMS {
            unweave_columns_invalid_unicode_not_printable_with_params(test_params);
        }
    }

    fn unweave_columns_invalid_unicode_end_of_line_fill_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"\xce\xb1\xce\n\xce\xb2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "α|β".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                b"\xce\xb1\xce   |\n     |\xce\xb2\n");
    }

    #[test]
    fn unweave_columns_invalid_unicode_end_of_line_fill() {
        for test_params in TEST_PARAMS {
            unweave_columns_invalid_unicode_end_of_line_fill_with_params(test_params);
        }
    }

    fn unweave_columns_invalid_unicode_end_of_line_wrap_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], b"\xce\xb1\xce\n\xce\xb2").unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "α|β".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(1),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                b"\xce\xb1|\n\xce|\n |\xce\xb2\n");
    }

    #[test]
    fn unweave_columns_invalid_unicode_end_of_line_wrap() {
        for test_params in TEST_PARAMS {
            unweave_columns_invalid_unicode_end_of_line_wrap_with_params(test_params);
        }
    }

    fn unweave_columns_tab_fill_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "αb\tc\nd".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "b|d".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(10),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                "αb      c |\n          |d\n".as_bytes());
    }

    #[test]
    fn unweave_columns_tab_fill() {
        for test_params in TEST_PARAMS {
            unweave_columns_tab_fill_with_params(test_params);
        }
    }

    fn unweave_columns_tab_wrap_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "αb\tc\nd".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "b|d".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::Expand(8),
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                "αb   |\n   c |\n     |d\n".as_bytes());
    }

    #[test]
    fn unweave_columns_tab_wrap() {
        for test_params in TEST_PARAMS {
            unweave_columns_tab_wrap_with_params(test_params);
        }
    }

    fn unweave_columns_tab_no_expand_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output");
        fs::write(&inputs[0], "αb\tc\nd".as_bytes()).unwrap();

        let opts = UnweaveOptionsColumns {
            pattern: "b|d".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
            width: UnweaveWidth::Column(5),
            column_separator: Some("|".to_string()),
            two_pass: test_params.two_pass,
            tab: UnweaveTab::NoExpand,
        };

        unweave_into_columns(&opts).unwrap();

        assert!(fs::read(&output).unwrap() ==
                "αb\tc  |\n     |d\n".as_bytes());
    }

    #[test]
    fn unweave_columns_tab_no_expand() {
        for test_params in TEST_PARAMS {
            unweave_columns_tab_no_expand_with_params(test_params);
        }
    }
}
