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

use crate::{UnweaveOptionsFiles, UnweaveError};
use crate::util::{TagFinder, FileLines};

use ahash::AHashMap;
use anyhow::{Result, Context, bail};

use std::io::{Write, BufWriter};
use std::fmt::Write as IoWrite;
use std::fs::File;
use std::path::Path;
use std::collections::hash_map::Entry;

/// Helper that creates and provides access to the output files.
///
/// The output files are created based on a template path provided during
/// OutputFiles creation. The template supports '%t' which is replaced by the
/// tag name and '%Nd' which is replaced with the stream number (starting from
/// 0) zero-padded to a length of N digits.
struct OutputFiles {
    template: String,
    writes: Vec<Box<dyn Write>>,
    write_for_tag_map: AHashMap<Vec<u8>, usize>,
    write_for_filename_map: AHashMap<String, usize>,
}

impl OutputFiles {
    /// Create a new OutputFiles struct with the specified output path template.
    fn new_for_template(template: &Path) -> Result<Self> {
        let output_files = OutputFiles {
            template: template.to_string_lossy().into_owned(),
            writes: Vec::new(),
            write_for_tag_map: AHashMap::new(),
            write_for_filename_map: AHashMap::new(),
        };

        // Create a dummy filename to catch invalid patterns early
        output_files.filename_for_tag("".as_bytes())?;

        Ok(output_files)
    }

    /// Gets the filename for a tag based on the path template
    /// this struct was created with.
    fn filename_for_tag(&self, tag: &[u8]) -> Result<String> {
        let count = self.writes.len().to_string();
        let mut fname = String::new();
        let mut inspecial = false;
        let mut width = 0;
        let tag = std::str::from_utf8(tag)?;

        for c in self.template.chars() {
            match (inspecial, c) {
                (false, '%') => {inspecial = true; width = 0; }
                (false, _) => fname.push(c),
                (true, '%') => { fname.push(c); inspecial = false; },
                (true, 't') => { fname.push_str(tag); inspecial = false; },
                (true, 'd') => {
                    write!(&mut fname, "{:0>1$}", count, width)?;
                    inspecial = false;
                },
                (true,  _) if c.is_ascii_digit() => { 
                    width = width * 10 + c.to_digit(10).unwrap() as usize;
                },
                _ => bail!(UnweaveError::InvalidOutputFilePattern(c)),
            }
        }

        if inspecial {
            bail!(UnweaveError::IncompleteOutputFilePattern);
        }

        Ok(fname)
    }

    /// Gets the Write objects for a tag, based on the path template
    /// this struct was created with.
    fn write_for_tag(&mut self, tag: &[u8]) -> Result<&mut dyn Write> {
        if let Some(w) = self.write_for_tag_map.get_mut(tag) {
            return Ok(&mut self.writes[*w]);
        }

        let filename = self.filename_for_tag(tag)?;
        let w = match self.write_for_filename_map.entry(filename.clone()) {
            Entry::Occupied(o) => *o.get(),
            Entry::Vacant(v) => {
                self.writes.push(Box::new(
                    BufWriter::new(
                        File::create(&filename).with_context(
                            || format!("Failed to create output file {}", filename)
                        )?
                    )
                ));
                *v.insert(self.writes.len() - 1)
            }
        };

        self.write_for_tag_map.insert(tag.to_vec(), w);

        return Ok(&mut self.writes[w]);
    }
}

/// Perform the unweave operation into multiple files, one file per matched stream.
pub(crate) fn unweave_into_files(opts: &UnweaveOptionsFiles) -> Result<()> {
    let mut output_files = OutputFiles::new_for_template(&opts.output.as_ref().unwrap())?;
    let mut tag_finder = TagFinder::new(&opts.pattern)?;

    for input in &opts.inputs {
        let mut file_lines = FileLines::new(input, opts.mmap)?;
        while let Some(line) = file_lines.next() {
            let tag = match tag_finder.find_in(&line) {
                Some(tag_range) => &line[tag_range],
                None => continue
            };
            let output_file = output_files.write_for_tag(tag)?;
            output_file.write(line)
                .and_then(|_| output_file.write(b"\n"))
                .with_context(
                    || format!("Failed to write to output file {}",
                                output_files.filename_for_tag(tag)
                                            .unwrap_or("<unknown>".to_string()))
                )?;
        }
    }

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempdir::TempDir;
    use std::fs::{self};
    use crate::UnweaveMmap;

    struct TestParams {
        mmap: UnweaveMmap,
    }

    const TEST_PARAMS: &[TestParams] = &[
        TestParams{ mmap: UnweaveMmap::Allow },
        TestParams{ mmap: UnweaveMmap::Disallow },
    ];

    fn unweave_into_files_with_tag_and_number_with_params(test_params: &TestParams) {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output-%t-%2d");
        fs::write(&inputs[0], b"A:1\nB:1\nA:2\nZ:1\nC:1\nB:2\nC:2").unwrap();

        let opts = UnweaveOptionsFiles {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: test_params.mmap,
        };

        unweave_into_files(&opts).unwrap();

        let output_a = tmpdir.path().join("output-A-00");
        let output_b = tmpdir.path().join("output-B-01");
        let output_c = tmpdir.path().join("output-C-02");

        assert!(fs::read(&output_a).unwrap() ==
                concat!("A:1\n",
                        "A:2\n").as_bytes());
        assert!(fs::read(&output_b).unwrap() ==
                concat!("B:1\n",
                        "B:2\n").as_bytes());
        assert!(fs::read(&output_c).unwrap() ==
                concat!("C:1\n",
                        "C:2\n").as_bytes());
    }

    #[test]
    fn unweave_into_files_with_tag_and_number() {
        for test_params in TEST_PARAMS {
            unweave_into_files_with_tag_and_number_with_params(test_params);
        }
    }

    #[test]
    fn unweave_into_files_incomplete_file_pattern() {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output-%t-%5");

        let opts = UnweaveOptionsFiles {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: UnweaveMmap::Allow,
        };

        assert!(unweave_into_files(&opts).is_err());
    }

    #[test]
    fn unweave_into_files_invalid_file_pattern() {
        let tmpdir = TempDir::new("unweave-test").unwrap();
        let inputs = vec![tmpdir.path().join("input1")];
        let output = tmpdir.path().join("output-%b");

        let opts = UnweaveOptionsFiles {
            pattern: "A|B|C".to_string(),
            output: Some(output.clone()),
            inputs: inputs,
            mmap: UnweaveMmap::Allow,
        };

        assert!(unweave_into_files(&opts).is_err());
    }
}
