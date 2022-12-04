unweave
=======

![](https://github.com/afrantzis/unweave/workflows/build/badge.svg)

unweave is a command-line tool to separate interleaved streams of text lines
into per-stream columns or files. Each line is classified based on a stream tag
extracted using a regular expression pattern.

### Documentation

Detailed information about the functionality and options of the unweave tool
can be found in the man page at:

  [doc/unweave.1.md](doc/unweave.1.md)

### Usage

Build with:

  `cargo build --release`

To run from within the build directory use either:

  `cargo run --release -- [OPTIONS...] PATTERN [FILE...]`

or directly with:

  `target/release/unweave [OPTIONS]... PATTERN [FILE]...`

To build and install along with the manpage use the `meson` build system:

  `meson setup --buildtype=release -Dstrip=true build`
  `ninja -C build`
  `ninja -C build install`

To install directly from crates.io:

  `cargo install unweave`

### Short intro

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

### License

This project is licensed under the GNU General Public License Version 3.0 or
later ([LICENSE](LICENSE) or https://gnu.org/licenses/gpl.html).
