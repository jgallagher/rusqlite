Pre-built sqlite bindings
=========================

This directory contains pre built (by rust-bindgen) bindings for various sqlite versions.

The general recipe for doing this is:
  1. Download sqlite amalgamation sources for the desired version (see links below)
  2. Expand the zip archive into a temporary directory
  3. Set environment variables `SQLITE3_LIB_DIR` and `SQLITE3_INCLUDE_DIR` to the location of the resulting source directory
  4. Build libsqlite3-sys with the feature `buildtime_bindgen` (e.g. `cargo build --features "buildtime_bindgen" -p libsqlite3-sys`)
  5. Copy `bindgen.rs` from within the `target` directory at the top level of the rusqlite workspace to an appropriate file in this directory (it will be found under `target/debug/build/libsqlite3-sys-*/out/bindgen.rs`)

Repeat the above process for each desired version, and also re-run each build using `--features "buildtime_bindgen,loadable_extension"` to generate the `-ext.h` versions to support sqlite3 loadable extensions.

sqlite3 amalgamation source links
---------------------------------
The location of the amalgamation sources used to build these are:
  - [3.7.16](https://sqlite.org/2013/sqlite-amalgamation-3071600.zip)
  - [3.7.7](https://sqlite.org/sqlite-amalgamation-3070700.zip)
  - [3.6.23](https://sqlite.org/sqlite-amalgamation-3_6_23.zip)
  - [3.6.8](https://sqlite.org/sqlite-amalgamation-3_6_8.zip)


