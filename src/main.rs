mod db;
mod index;
mod ocr;

use std::sync::OnceLock;
use std::{env, fs};

use anyhow::{anyhow, Context, Result};
use camino::Utf8PathBuf as PathBuf;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{arg, crate_description, crate_version, value_parser, ArgAction, Command};
use glob::Pattern;
use itertools::Itertools;

use crate::db::{SearchType, DB};
use crate::ocr::{Binarization, Ocr};

// reading those images eats so much memory
#[cfg(not(target_env = "msvc"))]
#[cfg(not(debug_assertions))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[cfg(not(debug_assertions))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

fn main() -> Result<()> {
    let matches = cli().get_matches();

    if matches.get_flag("dump-scan") {
        let mut o = Ocr::new(
            matches.get_one::<String>("lang").unwrap(),
            true,
            matches.get_one::<f32>("scale").copied(),
            matches.get_one::<Binarization>("binarization").copied(),
            matches.get_one::<i64>("psm").copied(),
        )?;
        let path = PathBuf::from(
            matches
                .get_one::<String>("QUERIES")
                .expect("queries shouldnt be empty"),
        );
        let res = o.scan(&path)?;
        println!("{}", res);
        return Ok(());
    }

    let dbpath: &PathBuf = matches.get_one("database").unwrap();
    if let Some(parent) = dbpath.parent() {
        if !parent.try_exists().expect("state dir should be exist") {
            fs::create_dir(parent).unwrap();
        }
    }

    if let Some(pwd) = matches.get_one::<String>("pwd") {
        env::set_current_dir(pwd).unwrap();
    }

    if let Some(threads) = matches.get_one::<usize>("threads") {
        let mut builder = rayon::ThreadPoolBuilder::new().num_threads(*threads);
        if *threads == 1 {
            builder = builder.use_current_thread();
        }
        builder.build_global().unwrap();
    }

    let mut exclude = Vec::from(["*/.cache", "*/.thumb*"].map(|x| Pattern::new(x).unwrap()));
    if let Some(patterns) = matches.get_many::<String>("exclude") {
        let mut parsed: Vec<Pattern> = patterns
            .map(|x| {
                Pattern::new(&format!("*/{x}"))
                    .with_context(|| format!("invalid pattern: {x}"))
                    .unwrap()
            })
            .collect();
        exclude.append(&mut parsed);
    }

    let scan_limit = matches.get_one::<usize>("scan-limit").copied();
    let debug = matches.get_flag("verbose");
    let max_size = matches.get_one::<String>("max-size").map(|x| {
        const ERR: &str = "invalid max-size: should be [width]x[height]";
        x.split('x')
            .map(|x| x.parse().expect(ERR))
            .collect_tuple::<(_, _)>()
            .expect(ERR)
    });

    let mut db = DB::new(dbpath)?;
    if matches.get_flag("index") {
        env::set_var("OMP_THREAD_LIMIT", "1");
        index::index_dir(
            &mut db,
            &PathBuf::try_from(env::current_dir().unwrap()).unwrap(),
            index::IndexOptions {
                lang: matches.get_one::<String>("lang").unwrap().to_owned(),
                debug,
                limit: scan_limit,
                exclude,
                rescan: matches.get_flag("rescan"),
                subdirs: matches.get_flag("subdirs"),
                chunksize: *matches.get_one::<usize>("chunk-size").unwrap(),
                cleanup: matches.get_flag("cleanup"),
                max_dimensions: max_size,
                scale: matches.get_one::<f32>("scale").copied(),
                binarization: matches.get_one::<Binarization>("binarization").copied(),
                psm: matches.get_one::<i64>("psm").copied(),
            },
        )?;
    }

    let queries = matches.get_many::<String>("QUERIES");
    if let Some(queries) = queries {
        let results = db.search(
            queries.map(|x| x.as_ref()).collect(),
            &PathBuf::try_from(env::current_dir().unwrap()).unwrap(),
            *matches.get_one::<usize>("limit").unwrap(),
            *matches.get_one::<SearchType>("search-type").unwrap(),
        )?;
        if cfg!(debug_assertions) && debug {
            println!("{:#?}", results)
        } else {
            for x in results {
                println!("{}\t{}", x.contents.escape_debug(), x.path);
            }
        }
    } else {
        return Err(anyhow!("No queries were provided"));
    }

    Ok(())
}

fn cli() -> Command {
    static DBPATH: OnceLock<PathBuf> = OnceLock::new();

    DBPATH
        .set(
            PathBuf::try_from(
                dirs::data_local_dir().expect("the user's local data dirctory should exist"),
            )
            .unwrap()
            .join("ocrlocate/index.db"),
        )
        .unwrap();

    Command::new("ocrlocate")
        .version(crate_version!())
        .about(crate_description!())
        .args([
            arg!(-d --database <FILE> "Location of the index database")
                .value_parser(value_parser!(PathBuf))
                .env("OCRLOCATE_DB")
                .default_value(DBPATH.get().unwrap().as_os_str()),
            arg!(--lang <LANG> "Tesseract language code")
                .default_value("eng")
                .long_help(
                    "Tesseract language identifier. Language package must be installed (such as tesseract-ocr-eng). Only affects
indexing of new images, so its recommended to delete the database when changed.",
                ),
            arg!(index: -n --"no-index" "Do not index the directory before searching, only search an existing index").action(ArgAction::SetFalse),
            arg!(-r --rescan "When indexing, ignore file modified time and force rescan"),
            arg!(-t --threads <THREADS> "Set threads").value_parser(value_parser!(usize)),
            arg!(-x --exclude <PATTERN> ... "Exclude directories and paths matching this pattern").long_help(
                "Exclude directories and paths matching a `glob` pattern: https://docs.rs/glob/latest/glob/struct.Pattern.html
Matched directories will not be descended into.  Excluded items will be removed from the index if --cleanup is specified."
            ),
            arg!(-m --"max-size" <RES> "Ignore images that are larger then [width]x[height]"),
            arg!(-c --cleanup "Delete files that no longer exist in the current directory from the index").conflicts_with("subdirs"),
            arg!(-v --verbose "Print debug messages"),
            arg!(-l --limit <LIMIT> "Max amount of results").value_parser(value_parser!(usize)).default_value("100"),
            arg!(subdirs: --"no-subdirs" "Do not recurse into subdirectories")
                .action(ArgAction::SetFalse),
            // maybe something for symlinks
            arg!(-s --"search-type" <TYPE> "Type of search query passed to the search index").default_value("simple").long_help(
                r#"Type of query to search. Default is to search for any instance of a literal value (`simple`)
`simple`: Passes sqlite fts5 the queries combined into one search phrase, i.e. `ocrlocate one two` matches "needleone twoneedle"
`match`: Passes sqlite fts5 the argument as an unescaped match query: https://www.sqlite.org/fts5.html#full_text_query_syntax.
    This is most useful if you are looking for a result that matches several terms anywhere, not just one long term
    Note that all queries are prefix queries with the tokenizer we use.
    Examples: `ocrlocate -s match one AND '"AND"'`
              `ocrlocate -s match needle NOT dontfind`
`glob`: Passes sqlite fts5 the argument as a glob query, which supports [a-z], *, and ?
    You will likely want to surround your query with *, due to the nature of OCR results
    The syntax is documented in here: https://sqlite.org/src/artifact?name=4204c561&ln=698
    To escape characters, include them in a set: [*], [[]
`regex`: Runs the regular expression on every row instead of using the index
    Uses the rust regex syntax https://docs.rs/regex/latest/regex/index.html#syntax"#
            ).value_parser(PossibleValuesParser::new(["simple", "match", "glob", "regex"]).map(|x| -> SearchType {
                match x.to_ascii_lowercase().as_str() {
                    "simple" => SearchType::Simple,
                    "match" => SearchType::Match,
                    "glob" => SearchType::Glob,
                    #[cfg(feature = "regex")] "regex" => SearchType::Regex,
                    #[cfg(not(feature = "regex"))] "regex" => panic!("This build was not compiled with regex support"),
                    _ => unreachable!()
                }
            })),
            arg!(--binarization <METHOD> "Which leptonica thresholding method to use")
                .value_parser(PossibleValuesParser::new(["Otsu", "LeptonicaOtsu", "Sauvola"]).map(|x| -> Binarization {
                    match x.as_str() {
                        "Otsu" => Binarization::Otsu,
                        "LeptonicaOtsu" => Binarization::LeptonicaOtsu,
                        "Sauvola" => Binarization::Sauvola,
                        _ => unreachable!()
                    }
                })),
            arg!(--psm <PSM> "Page segmentation mode").long_help(r#"Page segmentation mode
Documentation of values here: https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html#page-segmentation-method"#
            ).value_parser(value_parser!(i64).range(0..=13)).default_value("11"),
            // TODO: scale by max size, scale to res, etc
            arg!(--scale <FRAC> "Fraction to scale all images down by before applying ocr").value_parser(value_parser!(f32)),
            arg!(--pwd <PWD> "Set pwd").hide(true),
            arg!(--"scan-limit" <LIMIT> "Set max amount of scanned files")
                .hide(true)
                .value_parser(value_parser!(usize)),
            arg!(--"chunk-size" <SIZE> "Set chunk size")
                .hide(true)
                .value_parser(value_parser!(usize))
                .default_value("900"),
            arg!(--"dump-scan" "Dump the OCR result of one file and exit"),
            arg!(<QUERIES> ... "Strings to search for"),
        ])
}
