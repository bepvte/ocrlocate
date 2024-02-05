# ocrlocate
A tool for indexing a directory of images with optical character recognition for searching

## Dependencies
You will need tesseract language packs for your target language, and libleptonica-dev. If you arent installing it with --features bundled you will need libtesseract-dev too.

## Installation
Run `cargo install --git https://github.com/bepvte/ocrlocate`.

## Performance
To increase the performance by around 3-4 images per second, compile the bundled tesseract which should not use the slower OpenMP functions with `cargo install --git https://github.com/bepvte/ocrlocate -vv --features bundled`.
