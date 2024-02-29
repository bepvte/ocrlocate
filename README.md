# ocrlocate
A tool for indexing a directory of images with optical character recognition for searching

## Usage
```
Index and search a directory of images with OCR (optical character recognition)

Usage: ocrlocate [OPTIONS] <QUERIES>...

Arguments:
  <QUERIES>...
          Strings to search for

Options:
  -d, --database <FILE>
          Location of the index databse

          [env: OCRLOCATE_DB=]
          [default: /home/bep/.local/share/ocrlocate/index.db]

      --lang <LANG>
          Tesseract language identifier. Language package must be installed (such as tesseract-ocr-eng). Only affects
          indexing of new images, so its recommended to delete the database when changed.

          [default: eng]

  -n, --no-index
          Do not index the directory before searching, only search an existing index

  -r, --rescan
          When indexing, ignore file modified time and force rescan

  -t, --threads <THREADS>
          Set threads

  -x, --exclude <PATTERN>
          Exclude directories and paths matching a `glob` pattern: https://docs.rs/glob/latest/glob/struct.Pattern.html
          Matched directories will not be descended into.  Excluded items will be removed from the index if --cleanup is
          specified.

  -m, --max-size <RES>
          Ignore images that are larger then [width]x[height]

  -c, --cleanup
          Delete files that no longer exist in the current directory from the index

  -v, --verbose
          Print debug messages

  -l, --limit
          Max amount of results

      --no-subdirs
          Do not recurse into subdirectories

  -s, --search-type <TYPE>
          Type of query to search. Default is to search for any instance of a literal value (`simple`)
          `simple`: Passes sqlite fts5 the queries combined into one search phrase, i.e. `ocrlocate one two` matches
          "needleone twoneedle"
          `match`: Passes sqlite fts5 the argument as an unescaped match query:
          https://www.sqlite.org/fts5.html#full_text_query_syntax.
              Note that all queries are prefix queries with the tokenizer we use.
              Examples: `ocrlocate -s match one AND '"AND"'`
                        `ocrlocate -s match needle NOT dontfind`
          `glob`: Passes sqlite fts5 the argument as a glob query, which supports [a-z], *, and ?
              You will likely want to surround your query with *, due to the nature of OCR results
              The syntax is documented in here: https://sqlite.org/src/artifact?name=4204c561&ln=698
              To escape characters, include them in a set: [*], [[]
          `regex`: Runs the regular expression on every row instead of using the index
              Uses the rust regex syntax https://docs.rs/regex/latest/regex/index.html#syntax

          [default: simple]
          [possible values: simple, match, glob, regex]

      --dump-scan
          Dump the OCR result of one file and exit

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## Dependencies
You will need tesseract language packs for your target language, and libleptonica-dev. If you arent installing it with --features bundled you will need libtesseract-dev too.

## Installation
Run `cargo install --git https://github.com/bepvte/ocrlocate`.

## Performance
To increase the performance by around 3-4 images per second, compile the bundled tesseract which should not use the slower OpenMP functions with `cargo install --git https://github.com/bepvte/ocrlocate -vv --features bundled`.
