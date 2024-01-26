use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use itertools::Itertools;
use kdam::{BarBuilder, BarExt};
use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};

use crate::db::OcrResult;
use crate::{db::DB, ocr::Ocr};

type PathM = (PathBuf, fs::Metadata);

pub struct IndexOptions {
    pub lang: String,
    pub debug: bool,
    pub limit: Option<usize>,
    pub subdirs: bool,
    pub chunksize: usize,
}

pub fn index_dir(db: &mut DB, path: &Path, options: IndexOptions) -> Result<()> {
    let indexed_filetypes = ["png", "jpeg", "jpg", "gif", "webp"];
    let arcbar = Arc::new(Mutex::new(BarBuilder::default().total(0).build().unwrap()));

    let mut wd = WalkDir::new(path).follow_links(true);
    if !options.subdirs {
        wd = wd.max_depth(1);
    }

    let it = wd.into_iter().filter_map(|res| {
        let file = match res {
            Ok(file) => file,
            Err(e) => {
                println!("Error collecting files: {}", e);
                return None;
            }
        };
        if file.file_type().is_dir() {
            return None;
        };
        if let Some(ext) = file.path().extension() {
            if indexed_filetypes.contains(&ext.to_str().expect("paths should be valid unicode")) {
                return Some(file);
            }
        }
        return None;
    });

    let it: Box<dyn Iterator<Item = DirEntry>> = if let Some(limit) = options.limit {
        Box::new(it.take(limit))
    } else {
        Box::new(it)
    };

    let chunks = it.chunks(options.chunksize);

    for citer in chunks.into_iter() {
        let abar = arcbar.clone();
        let chunk: Vec<PathM> = citer
            .filter_map(move |file| match file.metadata() {
                Ok(metadata) => Some((file.path().to_owned(), metadata)),
                Err(e) => {
                    abar.lock()
                        .unwrap()
                        .write(format!("Error fetching metadata {}", e))
                        .unwrap();
                    None
                }
            })
            .collect();

        arcbar.clone().lock().unwrap().total += chunk.len();

        let abar = arcbar.clone();
        let chunk: Vec<PathM> = chunk
            .into_iter()
            .filter_map(|p| {
                if db.is_indexed(&p.0, &p.1) {
                    abar.lock().unwrap().update(1).unwrap();
                    return None;
                }
                return Some(p);
            })
            .collect();
        drop(abar);

        let abar = arcbar.clone();
        let (results, errors): (Vec<_>, Vec<_>) = chunk
            .par_iter()
            .map_init(
                || Ocr::new(&options.lang, options.debug).unwrap(),
                move |ocr, ele| {
                    if options.debug {
                        println!("now working on {}", &ele.0.to_str().unwrap());
                    }
                    let res = ocr.scan(&ele.0);
                    abar.lock().unwrap().update(1).unwrap();
                    return res.map(|x| OcrResult {
                        path: ele.0.clone(),
                        metadata: ele.1.clone(),
                        contents: x,
                    });
                },
            )
            .partition(Result::is_ok);

        arcbar.clone().lock().unwrap().clear().unwrap();

        for err in errors {
            println!("Error during ocr: {}", err.unwrap_err())
        }

        let results = results.into_iter().map(|x| x.unwrap()).collect_vec();

        let count = db.save_results(results)?;
        if options.debug {
            println!("Saved {count} to the db");
        }
    }
    Ok(())
}
