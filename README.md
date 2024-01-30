# OCRLocate
A tool for indexing a directory of images with optical character recognition for searching

## Dependencies
You will need tesseract language packs for your target language, and libleptonica-dev. If you arent installing it with --features bundled you will need libtesseract-dev too.

## Installation
Run `cargo install https://github.com/bepvte/ocrlocate`.

## Performance
To increase the performance, enable the compilation of tesseract without openmp with `cargo install https://github.com/bepvte/ocrlocate --features bundled`.