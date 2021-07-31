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

use crate::Result;
use crate::{UnweaveMmap, UnweaveTab};
use std::io::{BufRead, BufReader, Read, self, Seek, SeekFrom};
use std::fs::File;
use std::path::Path;
use memchr::memchr;
use unicode_segmentation::UnicodeSegmentation;

/// Finds stream tags with a regex pattern.
///
/// The first capture group (or the whole match if there is no capture group)
/// is used as the stream tag.
pub(crate) struct TagFinder {
    re: regex::bytes::Regex,
    capture_locations: regex::bytes::CaptureLocations,
}

impl TagFinder {
    /// Creates a new TagFinder with the specified regex pattern.
    pub(crate) fn new(pattern: &str) -> Result<TagFinder> {
        let re = regex::bytes::Regex::new(pattern)?;
        let capture_locations = re.capture_locations();
        Ok(TagFinder { re, capture_locations })
    }

    /// Finds the stream tag in a line.
    ///
    /// Returns the byte range of the tag within the line, or None if no
    /// tag was found.
    pub(crate) fn find_in(&mut self, line: &[u8]) -> Option<std::ops::Range<usize>> {
        self.re.captures_read(&mut self.capture_locations, &line)
            .map(|_| self.capture_locations
                         .get(self.capture_locations.len() - 1)
                         .map(|m| m.0..m.1)
            ).flatten()
    }
}

/// Iterator for the lines contained in a slice of [u8].
pub(crate) struct SliceFullLines<'a> {
    buf: &'a [u8],
    last: usize,
}

impl<'a> SliceFullLines<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        SliceFullLines { buf, last: 0 }
    }
}

impl<'a> Iterator for SliceFullLines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        match memchr(b'\n', &self.buf[self.last..]) {
            Some(m) => {
                let line = &self.buf[self.last..=(self.last + m)];
                self.last = self.last + m + 1;
                Some(line)
            },
            None => {
                let line = &self.buf[self.last..];
                if line.is_empty() {
                    None
                } else {
                    self.last = self.buf.len();
                    Some(line)
                }
            }
        }
    }
}

pub(crate) fn trim_newline(v: &[u8]) -> &[u8]
{
    let mut t = &v[..];
    if t.last() == Some(&b'\n') {
        t = &t[..t.len() - 1]
    }
    if t.last() == Some(&b'\r') {
        t = &t[..t.len() - 1]
    }
    t
}

/// Iterator like struct for the lines contained in a file, accessed using
/// memory mapping.
pub(crate) struct FileLinesMmap {
    mmap: memmap::Mmap,
    last: usize,
}

/// Iterator like struct for the lines contained in a file, accessed using
/// a BufRead object.
pub(crate) struct FileLinesBufreader {
    bufreader: BufReader<Box<dyn Read>>,
    buf: Vec<u8>,
}

impl FileLinesMmap {
    fn next(&mut self) -> Option<&[u8]> {
        match memchr(b'\n', &self.mmap[self.last..]) {
            Some(m) => {
                let line = &self.mmap[self.last..(self.last + m)];
                self.last = self.last + m + 1;
                Some(trim_newline(line))
            },
            None => {
                let line = &self.mmap[self.last..];
                if line.is_empty() {
                    None
                } else {
                    self.last = self.mmap.len();
                    Some(trim_newline(line))
                }
            }
        }
    }
}

impl FileLinesBufreader {
    fn next(&mut self) -> Option<&[u8]> {
        self.buf.clear();
        match self.bufreader.read_until(b'\n', &mut self.buf) {
            Ok(nread) if nread > 0 => Some(trim_newline(&self.buf)),
            _ => None
        }
    }
}

/// Iterator like struct for the lines contained in a file, abstracting
/// the method used to access the file data (mmap or BufRead).
pub(crate) enum FileLines {
    Mmap(FileLinesMmap),
    Bufreader(FileLinesBufreader),
}

/// Opens a file at the specified path. The path "/dev/stdin" is treated
/// specially, falling back to io::stdin() if normal open fails.
fn open_file(path: &Path) -> Result<Box<dyn Read>> {
    let res = File::open(path);
    if let Ok(f) = res {
        return Ok(Box::new(f));
    };

    if path.to_string_lossy() == "/dev/stdin" {
        return Ok(Box::new(io::stdin()));
    }

    Err(res.unwrap_err())?
}

impl FileLines {
    /// Creates a new FileLines object, backed by either mmap or BufRead
    /// depending on the path capabilities and user preference.
    pub(crate) fn new(path: &Path, mmap: UnweaveMmap) -> Result<Self> {
        if mmap == UnweaveMmap::Allow {
            let ret = Self::new_mmap(path);
            if ret.is_ok() {
                return ret;
            }
        }

        Self::new_bufreader(path)
    }

    /// Creates a new FileLines object, backed by mmap.
    fn new_mmap(path: &Path) -> Result<Self> {
        let mmap = unsafe { memmap::Mmap::map(&File::open(path)?)? };
        Ok(FileLines::Mmap(FileLinesMmap { mmap, last: 0 }))
    }

    /// Creates a new FileLines object, backed by a BufRead object.
    fn new_bufreader(path: &Path) -> Result<Self> {
        let bufreader = BufReader::new(open_file(path)?);
        Ok(FileLines::Bufreader(FileLinesBufreader { bufreader, buf: Vec::new() }))
    }

    /// Returns the next line, or None if there are no more lines.
    pub(crate) fn next(&mut self) -> Option<&[u8]> {
        match self {
            Self::Mmap(m) => m.next(),
            Self::Bufreader(b) => b.next(),
        }
    }
}

/// Provides access to file contents using mmap.
pub(crate) struct FileContentsMmap {
    mmap: memmap::Mmap,
}

/// Provides access to file contents by reading them to a buffer.
pub(crate) struct FileContentsBuf {
    buf: Vec<u8>,
}

/// Provides access to file contents, abstracting whether access
/// is through mmap or by having read the file to a buffer.
pub(crate) enum FileContents {
    Mmap(FileContentsMmap),
    Buf(FileContentsBuf),
}

impl FileContents {
    /// Creates a new FileContents object, backed by either mmap or buffer
    /// depending on the path capabilities and user preference.
    pub(crate) fn new(path: &Path, mmap: UnweaveMmap) -> Result<Self> {
        if !path.as_os_str().is_empty() && mmap == UnweaveMmap::Allow {
            let ret = Self::new_mmap(path);
            if ret.is_ok() {
                return ret;
            }
        }

        Self::new_buf(path)
    }

    /// Creates a new FileContents object, backed by mmap.
    fn new_mmap(path: &Path) -> Result<Self> {
        let mmap = unsafe { memmap::Mmap::map(&File::open(path)?)? };
        Ok(FileContents::Mmap(FileContentsMmap { mmap }))
    }

    /// Creates a new FileContents object, backed by a buffer.
    fn new_buf(path: &Path) -> Result<Self> {
        let mut reader = open_file(path)?;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(FileContents::Buf(FileContentsBuf { buf }))
    }

    /// Returns the file contents as byte slice.
    pub(crate) fn contents(&self) -> &[u8] {
        match self {
            Self::Mmap(m) => &*m.mmap,
            Self::Buf(b) => &b.buf,
        }
    }
}

/// Try to infer if the file at "path" can be reread. If seek fails or the file
/// offset is not the expected one assume that we can't reread.  Note that this
/// check may provide a false positive if the path is a device that fakes
/// successful seeks without actually seeking.
pub(crate) fn path_contents_can_be_reread(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    match file.seek(SeekFrom::Start(1)) {
        Ok(pos) if pos == 1 => {}
        _ => return false,
    };

    true
}

pub(crate) fn ascii_grapheme_count(b: u8) -> u32 {
    (b >= 0x20 && b != 0x7f) as u32
}

pub(crate) fn str_grapheme_count(grapheme: &str) -> u32 {
    if grapheme.len() > 1 {
        1
    } else {
        ascii_grapheme_count(grapheme.as_bytes()[0])
    }
}

pub(crate) fn grapheme_count_tab_expanded(line: &[u8], tab: UnweaveTab,
                                          mut out: Option<&mut Vec<u8>>) -> u32 {
    let mut grapheme_count: u32 = 0;

    for_each_grapheme(line,
        |g| {
            match g {
                Grapheme::Unicode(s) => {
                    match (s, tab) {
                        ("\t", UnweaveTab::Expand(tw)) => {
                            let nspaces = tw - grapheme_count % tw;
                            if let Some(out) = &mut out {
                                out.extend(std::iter::repeat(b' ').take(nspaces as usize));
                            }
                            grapheme_count += nspaces;
                        }
                        _ => {
                            if let Some(out) = &mut out {
                                out.extend_from_slice(s.as_bytes());
                            }
                            grapheme_count += str_grapheme_count(s);
                        }
                    }
                },
                Grapheme::Ascii(b) => {
                    match (b, tab) {
                        (b'\t', UnweaveTab::Expand(tw)) => {
                            let nspaces = tw - grapheme_count % tw;
                            if let Some(out) = &mut out {
                                out.extend(std::iter::repeat(b' ').take(nspaces as usize));
                            }
                            grapheme_count += nspaces;
                        }
                        _ => {
                            if let Some(out) = &mut out {
                                out.push(b);
                            }
                            grapheme_count += ascii_grapheme_count(b);
                        }
                    }
                }
            }
            Ok(())
        }
    ).unwrap();

    grapheme_count
}

pub(crate) enum Grapheme<'a> {
    Ascii(u8),
    Unicode(&'a str)
}

pub(crate) fn for_each_grapheme(line: &[u8],
                                mut callback_fn: impl FnMut(Grapheme)->Result<()>)
    -> Result<()>
{
    let mut cur = line;

    if line.is_ascii() {
        for b in line {
            callback_fn(Grapheme::Ascii(*b))?;
        }
        return Ok(());
    }

    loop {
        let (valid, invalid) = match std::str::from_utf8(cur) {
            Ok(s) => (s, None),
            Err(e) => {
                let (valid, after_valid) = cur.split_at(e.valid_up_to());
                let valid_str = unsafe { std::str::from_utf8_unchecked(valid) };
                let invalid_len = match e.error_len() {
                    Some(l) => l,
                    None => after_valid.len(),
                };
                cur = &after_valid[invalid_len..];
                (valid_str, Some(&after_valid[..invalid_len]))
            }
        };

        // Iterate over the valid part, one grapheme at a time
        for grapheme in valid.graphemes(true) {
            callback_fn(Grapheme::Unicode(grapheme))?;
        }

        if invalid.is_none() {
            break;
        }

        // Iterate over the invalid part, one byte at a time
        for b in invalid.unwrap() {
            callback_fn(Grapheme::Ascii(*b))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tabs_ascii() {
        let mut out = Vec::new();
        let ngraphemes = grapheme_count_tab_expanded(b"ab\tcdefghijk\tl\t",
                                                     UnweaveTab::Expand(8),
                                                     Some(&mut out));

        let expected = b"ab      cdefghijk       l       ";
        assert!(ngraphemes == expected.len() as u32);
        assert!(out == expected);
    }

    #[test]
    fn expand_tabs_unicode() {
        let mut out = Vec::new();
        let ngraphemes = grapheme_count_tab_expanded("αβ\tγδεζηθικλ\tμ\t".as_bytes(),
                                                     UnweaveTab::Expand(8),
                                                     Some(&mut out));

        let expected = "αβ      γδεζηθικλ       μ       ";
        assert!(ngraphemes == expected.chars().count() as u32);
        assert!(out == expected.as_bytes());
    }
}
