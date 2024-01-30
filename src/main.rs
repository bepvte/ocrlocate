mod db;
mod index;
mod ocr;

use std::path::PathBuf;
use std::{env, fs};

use anyhow::{anyhow, Result};
use clap::{arg, crate_description, crate_version, value_parser, Arg, Command};
use dirs::data_local_dir;
use glob::Pattern;
use itertools::Itertools;

use crate::db::DB;
use crate::ocr::Ocr;

// reading those images eats so much memory
#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

fn main() -> Result<()> {
    let matches = cli().get_matches();

    let lang: String = matches.get_one::<String>("lang").unwrap().to_owned();

    if matches.get_flag("dump-scan") {
        let mut o = Ocr::new(&lang, true)?;
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
            .map(|x| Pattern::new(&format!("*/{}", x)).expect("invalid pattern"))
            .collect();
        exclude.append(&mut parsed);
    }

    let scan_limit = matches.get_one::<usize>("scan-limit").map(|x| *x);
    let debug = matches.get_flag("verbose");
    let max_size = matches.get_one::<String>("max-size").map(|x| {
        const ERR: &str = "invalid max-size: should be widthxheight";
        x.split('x')
            .map(|x| x.parse().expect(ERR))
            .collect_tuple::<(_, _)>()
            .expect(ERR)
    });

    let mut db = DB::new(dbpath)?;
    if !matches.get_flag("no-index") {
        env::set_var("OMP_THREAD_LIMIT", "1");
        index::index_dir(
            &mut db,
            &env::current_dir().expect("current dir should be accessible"),
            index::IndexOptions {
                lang,
                debug,
                limit: scan_limit,
                exclude,
                rescan: matches.get_flag("rescan"),
                subdirs: matches.get_flag("no-subdirs"),
                chunksize: *matches.get_one::<usize>("chunk-size").unwrap(),
                cleanup: matches.get_flag("cleanup"),
                max_dimensions: max_size,
            },
        )?;
    }

    let queries = matches.get_many::<String>("QUERIES");
    if let Some(queries) = queries {
        let results = db.search(
            queries.map(|x| x.as_ref()).collect(),
            *matches.get_one::<usize>("limit").unwrap(),
        )?;
        if debug {
            println!("{:#?}", results)
        } else {
            for x in results {
                println!("{}\t\t{}", x.contents.escape_debug(), x.path);
            }
        }
    } else {
        return Err(anyhow!("No queries were provided"));
    }

    Ok(())
}

fn cli() -> Command {
    let dbpath = data_local_dir()
        .expect("the user's local data dirctory should exist")
        .join("ocrlocate/index.db");
    Command::new("ocrlocate")
        .version(crate_version!())
        .about(crate_description!())
        .args([
            arg!(-d --database <FILE> "Location of the index databse")
                .value_parser(value_parser!(PathBuf))
                .env("OCRLOCATE_DB")
                .default_value(dbpath.into_os_string()),
            arg!(-l --lang <LANG> "Tesseract language code")
                .default_value("eng")
                .long_help(
                    "Tesseract language identifier. Language package must be installed (such as tesseract-ocr-eng). Only affects
indexing of new images, so its recommended to delete the database when changed.",
                )
                .value_parser(|e: &str| -> Result<String, &'static str> {
                    if e.len() != 3 || e.contains(['.', '/', '\\']) || !e.is_ascii() {
                        return Err("Invalid language code");
                    }
                    Ok(e.to_owned())
                }),
            arg!(-n --"no-index" "Do not index the directory first"),
            arg!(-r --"rescan" "Ignore file modified time and force rescan"),
            arg!(-t --threads <THREADS> "Set threads").value_parser(value_parser!(usize)),
            arg!(-x --exclude <PATTERN> ... "Exclude directories and paths matching this pattern").long_help(
                "Exclude directories and paths matching a `glob` pattern: https://docs.rs/glob/latest/glob/struct.Pattern.html
Matched directories will not be descended into.
Excluded items will be unindexed until someone fixes that."
            ),
            arg!(-m --"max-size" <RES> "Ignore images that are larger then WIDTHxHEIGHT"),
            arg!(-c --cleanup "Delete files that no longer exist in the current directory from the index").conflicts_with("no-subdirs"),
            arg!(-v --verbose "Print debug messages"),
            arg!(-l --limit "Max amount of results").value_parser(value_parser!(usize)).default_value("100"),
            arg!(--"no-subdirs" "Do not recurse into subdirectories")
                .action(clap::ArgAction::SetFalse),
            // maybe something for symlinks
            arg!(--pwd <PWD> "Set pwd").hide(true),
            arg!(--scan-limit <LIMIT> "Set limit")
                .hide(true)
                .value_parser(value_parser!(usize)),
            arg!(--"chunk-size" <SIZE> "Set chunk size")
                .hide(true)
                .value_parser(value_parser!(usize))
                .default_value("900"),
            arg!(--"dump-scan" "Dump an OCR result and exit").hide(true),
            Arg::new("QUERIES")
                .num_args(..)
                .trailing_var_arg(true)
                .help("Strings to search for"),
        ])
}
