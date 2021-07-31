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

mod unweave_columns;
mod unweave_files;
mod util;

use anyhow::{Result, bail};
use getopts::Options;
use unweave_columns::unweave_into_columns;
use unweave_files::unweave_into_files;
use std::path::{PathBuf, Path};
use std::fmt;
use std::error::Error;

#[derive(Debug)]
enum UnweaveError {
    ParsingFailure(String),
    MissingOption(&'static str),
    InvalidOption(&'static str),
    InvalidOptionValue(&'static str, String),
    InvalidTwoPassReread,
    LineAndColumnWidth,
    InvalidOutputFilePattern(char),
    IncompleteOutputFilePattern,
}

impl fmt::Display for UnweaveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ParsingFailure(e) =>
                write!(f, "{}", e),
            Self::MissingOption(o) =>
                write!(f, "Missing required option '{}'", o),
            Self::InvalidOption(o) =>
                write!(f, "Invalid option '{}' for selected mode", o),
            Self::InvalidOptionValue(o, v) =>
                write!(f, "Invalid value '{}' for option '{}'", v, o),
            Self::InvalidTwoPassReread =>
                write!(f, "Cannot use two-pass mode reread for the specified inputs"),
            Self::LineAndColumnWidth =>
                write!(f, "Cannot specify both --line-width and --column-width"),
            Self::InvalidOutputFilePattern(c) =>
                write!(f, "Invalid character '{}' in output file pattern", c),
            Self::IncompleteOutputFilePattern =>
                write!(f, "Incomplete output file pattern"),
        }
    }
}

impl Error for UnweaveError {}

#[derive(PartialEq, Copy, Clone)]
enum UnweaveTwoPass { Cached, Reread }
#[derive(PartialEq)]
enum UnweaveWidth { Undefined, Column(u32), Line(u32) }

impl UnweaveWidth {
    fn is_column(&self) -> bool {
        if let Self::Column(_) = self { true } else { false }
    }
}

#[derive(Copy, Clone, PartialEq)]
enum UnweaveMmap { Allow, Disallow }

#[derive(Copy, Clone, PartialEq)]
enum UnweaveTab { NoExpand, Expand(u32) }

impl UnweaveTab {
    fn is_expand(&self) -> bool {
        if let Self::Expand(_) = self { true } else { false }
    }
}

struct UnweaveOptionsColumns {
    pattern: String,
    output: Option<PathBuf>,
    inputs: Vec<PathBuf>,
    mmap: UnweaveMmap,
    width: UnweaveWidth,
    column_separator: Option<String>,
    two_pass: UnweaveTwoPass,
    tab: UnweaveTab,
}

struct UnweaveOptionsFiles {
    pattern: String,
    output: Option<PathBuf>,
    inputs: Vec<PathBuf>,
    mmap: UnweaveMmap,
}

enum UnweaveOptions {
    Columns(UnweaveOptionsColumns),
    Files(UnweaveOptionsFiles),
}

fn parse_options(args: &[impl AsRef<std::ffi::OsStr>]) -> Result<UnweaveOptions> {
    let mut opts = Options::new();
    opts
        .optopt(
            "m", "mode",
            concat!(
                "the unweave output mode, into separate columns in a single file ",
                "(\"columns\", the default), or separate files (\"files\")"
            ),
            "MODE"
        )
        .optopt(
            "c", "column-width",
            "the width, in characters, of each column in the output (for columns mode)",
            "COLUMN-WIDTH",
        )
        .optopt(
            "l", "line-width",
            concat!(
                "the width, in characters, of each line in the output (for ",
                "columns mode), with all columns having the same automatically ",
                "calculated width"
            ),
            "LINE-WIDTH",
        )
        .optopt(
            "s", "column-separator",
            "the separator to print between columns in the output (for columns mode)",
            "COLUMN-SEPARATOR",
        )
        .optopt(
            "", "two-pass",
            concat!(
                "when a second pass through the data is required, either use the data ",
                "and other information stored in memory from the first pass(\"cached\", ",
                "the default), or reread and reprocess the data (\"reread\")"
            ),
            "PASS-MODE",
        )
        .optflag(
            "n", "no-mmap",
            "do not use mmap to access file contents"
        )
        .optopt(
            "o", "output",
            concat!(
                "output file (for columns mode), or an output file template (for files mode) ",
                "in which '%t' is replaced with the stream tag and '%Nd' with the stream ",
                "number (starting from 0) zero-padded to a length of N digits"
            ),
            "OUTPUT"
        )
        .optopt(
            "t", "tab-width",
            concat!(
                "in columns mode, the number of spaces to replace tab characters with ",
                "(default: 8), or \"noexpand\" to disable tab expansion"
            ),
            "TAB-WIDTH"
        )
        .optflag(
            "", "version",
            "output version information and exit"
        )
        .optflag(
            "h", "help",
            "display this help and exit"
        );

    let matches = opts.parse(args).map_err(|e| UnweaveError::ParsingFailure(e.to_string()))?;

    if matches.opt_present("help") {
        print!("{}",
            opts.usage(
                concat!(
                    "Usage: unweave [OPTION...] PATTERN [FILE..]\n",
                    "Unweave interleaved streams of text lines using regular expression matching.\n",
                    "\n",
                    "Each line is classified based on a stream tag extracted using the regular\n",
                    "expression PATTERN. The first capture group (or the whole match if there is no\n",
                    "explicit capture group) is used as the stream tag for the match. Without a \n",
                    "FILE, or when FILE is -, read standard input.",
                )
            )
        );
        std::process::exit(0);
    }

    if matches.opt_present("version") {
        print!(
            concat!(
                "unweave 1.0.0\n",
                "Copyright (C) 2022 Alexandros Frantzis\n",
                "License GPL-3.0-or-later <https://gnu.org/licenses/gpl.html>.\n"
            )
        );
        std::process::exit(0);
    }

    let pattern = match matches.free.first() {
        None => bail!(UnweaveError::MissingOption("pattern")),
        Some(p) if p.is_empty() => 
            bail!(UnweaveError::InvalidOptionValue("pattern", p.to_string())),
        Some(p) => p.to_string(),
    };

    // Input from stdin (either no input file or "-") is marked with the
    // "/dev/stdin" filename. This allows us to open the file on systems that
    // support it, potentially getting direct access to the underlying file in
    // case of redirection. On systems where the file doesn't exist we fall back
    // to using io::stdin (see util::open_file()).
    let mut inputs: Vec<_> = matches.free
        .iter()
        .skip(1)
        .map(|m| PathBuf::from(if m == "-" { "/dev/stdin" } else { m }))
        .collect();
    if inputs.is_empty() {
        inputs.push(PathBuf::from("/dev/stdin"));
    }

    let mode = matches.opt_str("mode").unwrap_or("columns".to_string());
    match mode.as_str() {
        "columns" | "files" => {},
        _ => bail!(UnweaveError::InvalidOptionValue("mode", mode)),
    };

    if mode == "files" {
        if !matches.opt_present("output") {
            return Err(UnweaveError::MissingOption("output").into());
        }
        for opt in &["line-width", "column-width", "two-pass", "tab-width"] {
            if matches.opt_present(opt) {
                bail!(UnweaveError::InvalidOption(opt));
            }
        }
    }

    if matches.opt_present("line-width") && matches.opt_present("column-width") {
        bail!(UnweaveError::LineAndColumnWidth);
    }

    let width = 
        if matches.opt_present("line-width") {
            match matches.opt_get::<u32>("line-width") {
                Ok(Some(lw)) if lw > 0 => UnweaveWidth::Line(lw),
                _ => bail!(
                    UnweaveError::InvalidOptionValue(
                        "line-width",
                        matches.opt_str("line-width").unwrap_or("".to_string())
                    )
                ),
            }
        } else if matches.opt_present("column-width") {
            match matches.opt_get::<u32>("column-width") {
                Ok(Some(cw)) if cw > 0 => UnweaveWidth::Column(cw),
                _ => bail!(
                    UnweaveError::InvalidOptionValue(
                        "column-width",
                        matches.opt_str("column-width").unwrap_or("".to_string())
                    )
                ),
            }
        } else {
            UnweaveWidth::Undefined
        };

    let two_pass = matches.opt_str("two-pass").unwrap_or("cached".to_string());
    let two_pass = match two_pass.as_str() {
        "cached" => UnweaveTwoPass::Cached,
        "reread" => UnweaveTwoPass::Reread,
        _ => bail!(UnweaveError::InvalidOptionValue("two-pass", two_pass)),
    };

    if two_pass == UnweaveTwoPass::Reread &&
        inputs.iter().any(|f| !util::path_contents_can_be_reread(Path::new(f)))
    {
        bail!(UnweaveError::InvalidTwoPassReread);
    }

    let mmap = if matches.opt_present("no-mmap") { 
        UnweaveMmap::Disallow
    } else { 
        UnweaveMmap::Allow
    };

    let tab = match matches.opt_get::<u32>("tab-width") {
        Ok(None) => UnweaveTab::Expand(8),
        Ok(Some(tw)) if tw > 0 => UnweaveTab::Expand(tw),
        _ => { 
            if matches.opt_str("tab-width") == Some("noexpand".to_string()) {
                UnweaveTab::NoExpand
            } else {
                bail!(
                    UnweaveError::InvalidOptionValue(
                        "tab-width",
                        matches.opt_str("tab-width").unwrap_or("".to_string())
                    )
                )
            }
        }
    };

    match mode.as_str() {
        "columns" => {
            Ok(
                UnweaveOptions::Columns(UnweaveOptionsColumns {
                    pattern: pattern,
                    output: matches.opt_str("output").map(PathBuf::from),
                    inputs: inputs,
                    mmap: mmap,
                    width: width,
                    column_separator: matches.opt_str("column-separator"),
                    two_pass: two_pass,
                    tab: tab,
                })
            )
        },
        "files" => {
            Ok(
                UnweaveOptions::Files(UnweaveOptionsFiles {
                    pattern: pattern,
                    output: matches.opt_str("output").map(PathBuf::from),
                    inputs: inputs,
                    mmap: mmap,
                })
            )
        },
        _ => panic!(),
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let opts = parse_options(&args[1..])?;

    Ok(
        match &opts {
            UnweaveOptions::Files(o) => unweave_into_files(o),
            UnweaveOptions::Columns(o) => unweave_into_columns(o),
        }?
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_require_pattern() {
        let opts = parse_options(&["--mode=columns"]);
        assert!(opts.is_err());
        let opts = parse_options(&["--mode=columns", ""]);
        assert!(opts.is_err());
        let opts = parse_options(&["--mode=columns", "bla"]);
        assert!(opts.is_ok());
    }

    #[test]
    fn options_require_output_for_files() {
        let opts = parse_options(&["--mode=files", "bla"]);
        assert!(opts.is_err());
    }

    #[test]
    fn options_for_files_do_not_accept_width() {
        let opts = parse_options(&["--mode=files", "--column-width=10", "bla"]);
        assert!(opts.is_err());
        let opts = parse_options(&["--mode=files", "--line-width=10", "bla"]);
        assert!(opts.is_err());
    }

    #[test]
    fn options_do_not_accept_both_column_and_line_width() {
        let opts = parse_options(&["--mode=files", "--column-width=10",
                                   "--line-width=10", "bla"]);
        assert!(opts.is_err());
    }

    #[test]
    fn options_processed_for_columns() {
        let opts = parse_options(&["--mode=columns", "--column-width=10", "--output=output1",
                                   "--tab-width=3", "bla", "input1", "input2"]).unwrap();
        let opts = if let UnweaveOptions::Columns(o) = opts { o } else { panic!("") };
        assert!(opts.pattern == "bla");
        assert!(opts.output == Some(PathBuf::from("output1")));
        assert!(opts.width == UnweaveWidth::Column(10));
        assert!(opts.inputs == [PathBuf::from("input1"), PathBuf::from("input2")]);
        assert!(opts.mmap == UnweaveMmap::Allow);
        assert!(opts.tab == UnweaveTab::Expand(3));
    }

    #[test]
    fn options_processed_for_files() {
        let opts = parse_options(&["--mode=files", "--output=output1",
                                   "bla", "input1", "input2"]).unwrap();
        let opts = if let UnweaveOptions::Files(o) = opts { o } else { panic!("") };
        assert!(opts.pattern == "bla");
        assert!(opts.output == Some(PathBuf::from("output1")));
        assert!(opts.inputs == [PathBuf::from("input1"), PathBuf::from("input2")]);
        assert!(opts.mmap == UnweaveMmap::Allow);
    }

    #[test]
    fn options_input_from_stdin_adds_dev_stdin() {
        let opts = parse_options(&["--mode=files", "--output=output1", "bla"]).unwrap();
        let opts = if let UnweaveOptions::Files(o) = opts { o } else { panic!("") };
        assert!(opts.pattern == "bla");
        assert!(opts.output == Some(PathBuf::from("output1")));
        assert!(opts.inputs == [PathBuf::from("/dev/stdin")]);
    }

    #[test]
    fn options_no_mmap() {
        let opts = parse_options(&["--no-mmap", "bla"]).unwrap();
        let opts = if let UnweaveOptions::Columns(o) = opts { o } else { panic!("") };
        assert!(opts.mmap == UnweaveMmap::Disallow);
    }

    #[test]
    fn options_short() {
        let opts = parse_options(&["-m", "columns", "-c", "10", "-o", "output1",
                                   "-t", "7", "-n", "bla"]).unwrap();
        let opts = if let UnweaveOptions::Columns(o) = opts { o } else { panic!("") };
        assert!(opts.pattern == "bla");
        assert!(opts.output == Some(PathBuf::from("output1")));
        assert!(opts.width == UnweaveWidth::Column(10));
        assert!(opts.inputs == [PathBuf::from("/dev/stdin")]);
        assert!(opts.mmap == UnweaveMmap::Disallow);
        assert!(opts.tab == UnweaveTab::Expand(7));
    }

    #[test]
    fn options_tab_noexpand() {
        let opts = parse_options(&["--tab-width=noexpand", "bla"]).unwrap();
        let opts = if let UnweaveOptions::Columns(o) = opts { o } else { panic!("") };
        assert!(opts.tab == UnweaveTab::NoExpand);
    }
}
