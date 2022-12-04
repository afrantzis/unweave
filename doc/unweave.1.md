% unweave(1) 1.0.0
%
% December 2022

NAME
====

unweave - unweave interleaved streams of text lines using regular experssion
matching

SYNOPSIS
========

unweave [OPTION]... PATTERN [FILE]...

DESCRIPTION
===========

unweave is a command-line tool to separate interleaved streams of text lines
into per-stream columns or files. Each line is classified based on a stream tag
extracted using the regular expression PATTERN. The first capture group (or the
whole match if there is no explicit capture group) is used as the stream tag
for the match.

Input is read sequentially from the FILEs specified on the command line, or from
standard input if no files are provided. The special file name "-" denotes
standard input.

In columns mode (see **\-\-mode**), output is written to standard out unless
directed to a different file with the **\-\-output** option. In files mode, the
use of the **\-\-output** option, containing an output file template, is required.

OPTIONS
=======

`-m, --mode MODE`

: the unweave output mode, into separate columns in a single file
  ("columns", the default), or separate files ("files")

`-c, --column-width COLUMN-WIDTH`

: the width, in characters, of each column in the output (for columns mode)

`-l, --line-width LINE-WIDTH`

: the width, in characters, of each line in the output (for columns mode), with
  all columns having the same automatically calculated width

`-s, --column-separator COLUMN-SEPARATOR`

: the separator to print between columns in the output (for columns mode)

`--two-pass PASS-MODE`

: when a second pass through the data is required, either use the data
  and other information stored in memory from the first pass ("cached",
  the default), or reread and reprocess the data ("reread")

`-n, --no-mmap`

: do not use mmap to access file contents

`-o, --output OUTPUT`

: output file (for columns mode), or an output file template (for files mode)
  in which '%t' is replaced with the stream tag and '%Nd' with the stream
  number (starting from 0) zero-padded to a length of N digits

`--tab-width TAB-WIDTH`

: in columns mode, the number of spaces to replace tab characters with (default: 8),
  or \"noexpand\" to disable tab expansion

`--version`

: output version information and exit

`-h, --help`

: display this help and exit

NUMBER OF PASSES
================

Most combinations of options in columns mode require two passes through the
data: a first pass to calculate the number and widths of columns, and a second
pass to print out the matched lines into their proper column. All of the input
data needs to be read before any output is produced.

The one combination that allows for a single pass is when the column width is
explicitly specified (**\-\-column-width W** option) and there is no column
separator (no **\-\-column-separator** option).

Unweave in files mode always uses a single pass.

When using a single pass, unweave is able to act as a streaming filter,
producing output after every matching input line.

TAB EXPANSION
=============

In columns mode, TAB characters are expanded to spaces in order to be able to
properly fill and wrap the column contents. The default number of spaces used
for tab expansion is 8, and can be changed with the **\-\-tab TAB** option.

To disable tab expansion use **\-\-tab noexpand**. Note that disabling tab
expansion is likely to cause column formatting issues.

In files mode, tabs are not expanded.

CONTROL CHARACTERS AND INVALID UTF-8 INPUT
==========================================

The primary focus of unweave is valid UTF-8 text data without control
characters (with the exception of TAB which is treated specially). Unweave will
process and output all input it receives, but lines containing control
characters are likely to not be properly aligned in columns mode (depending on
the editor or console the output is viewed with).

Invalid UTF-8 input sequences are handled on a best-effort basis in terms of
column alignment, with each invalid byte treated as a single, extended ASCII
grapheme for the purposes of columnization.

REDUCING MEMORY CONSUMPTION
===========================

There are two main sources of significant memory consumption in unweave: memory
mapping, and caching when performing two passes. Both of these are used by
default (when possible) since they tend to offer considerable performance
gains.

Memory mapping will eventually cause to the whole file to be mapped into
memory. To avoid this, the **\-\-no-mmap** option can be specified to use buffered
line reads instead.

When two passes are required, the default ("cached" two pass mode) is to
perform a first pass that loads the file data into memory (with mmap if
possible) and preprocesses it to extract line and column information. A second
pass uses the loaded data and cached information to print the columns. To avoid
keeping the extra information in memory, the "reread" two pass mode can be used:
a first pass extracts minimal column info, while the second pass rereads the
data and prints the columns. Since this approach needs to reread the data, it
cannot be used when reading input from streaming input sources like stdin.

Note that **\-\-two-pass reread** will still mmap file contents if possible, so the
suggestion to use the **\-\-no-mmap** flag still applies.

EXAMPLES
========

Consider the input file consisting of lines from three streams, tagged A, B and
C:

```
[info] A: 1
[info] A: 2
[info] B: 1
[error] A: 3
[info] B: 2
[error] C: 1
```

Unweaving with the regex pattern `A|B|C` into columns gives:

```
$ unweave 'A|B|C' input
[info] A: 1
[info] A: 2
            [info] B: 1
[error] A: 3
            [info] B: 2
                       [error] C: 1
```

The width of each column and column separator can be set:

```
$ unweave -c 15 -s '|' 'A|B|C' input
[info] A: 1    |               |
[info] A: 2    |               |
               |[info] B: 1    |
[error] A: 3   |               |
               |[info] B: 2    |
               |               |[error] C: 1
```

For more complex cases the first capture group of the regex can be used to
acquire the stream tag. Also the total output line width can be set, which will
be equally split among the columns:

```
$ unweave -l 45 '] ([A-Z]): \d' input
[info] A: 1
[info] A: 2
               [info] B: 1
[error] A: 3
               [info] B: 2
                              [error] C: 1
```

To unweave into separate files with each output filename containing the stream
tag and the stream id:

```
$ unweave --mode=files -o 'stream-%t-%2d' 'A|B|C' input
$ tail -n +1 stream-*
==> stream-A-00 <==
[info] A: 1
[info] A: 2
[error] A: 3

==> stream-B-01 <==
[info] B: 1
[info] B: 2

==> stream-C-02 <==
[error] C: 1
```

COPYRIGHT
=========

Copyright 2022 Alexandros Frantzis. License GPL-3.0-or-later: GNU GPL version 3
or later <https://gnu.org/licenses/gpl.html>
