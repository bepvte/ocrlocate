use std::path::Path;
use std::sync::{Arc, Mutex};
use std::{env, iter};

use anyhow::Result;
use glob::Pattern;
use image::io::Reader as ImageReader;
use itertools::{Either, Itertools};
use kdam::{BarBuilder, BarExt};
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::db::OcrResult;
use crate::{db::DB, ocr::Ocr};

pub struct IndexOptions {
    pub lang: String,
    pub debug: bool,
    pub limit: Option<usize>,
    pub exclude: Vec<Pattern>,
    pub rescan: bool,
    pub subdirs: bool,
    pub chunksize: usize,
    pub cleanup: bool,
    pub max_dimensions: Option<(u32, u32)>,
}

pub fn index_dir(db: &mut DB, path: &Path, options: IndexOptions) -> Result<()> {
    let indexed_filetypes = ["png", "jpeg", "jpg", "gif", "webp"];

    let mut wd = WalkDir::new(path).follow_links(true);
    if !options.subdirs {
        wd = wd.max_depth(1);
    }

    let it = wd
        .into_iter()
        .filter_entry(|entry| !options.exclude.iter().any(|x| x.matches_path(entry.path())))
        .filter_map(|res| {
            let file = match res {
                Ok(file) => file,
                Err(e) => {
                    eprintln!("[Error] collecting files: {}", e);
                    return None;
                }
            };
            if file.file_type().is_dir() {
                return None;
            };
            if let Some(ext) = file.path().extension() {
                if indexed_filetypes.contains(&ext.to_str().expect("paths should be valid unicode"))
                {
                    return Some(file);
                }
            }
            return None;
        });

    let it = if let Some(limit) = options.limit {
        Either::Left(it.take(limit))
    } else {
        Either::Right(it)
    };

    if options.cleanup {
        db.mark_for_deletion(&env::current_dir().unwrap());
    }

    let arcbar = Arc::new(Mutex::new(BarBuilder::default().total(0).build().unwrap()));

    // the chunking starves the rayon pool but its fine
    let chunks = it.chunks(options.chunksize);
    let mut tup = chunks
        .into_iter()
        .map(|x| x.collect())
        .chain(iter::once(vec![]))
        .tuple_windows::<(_, _)>();

    let mut first_iter = true;
    while let Some((c1, c2)) = tup.next() {
        let abar = arcbar.clone();
        let chunk: Vec<_> = c1
            .into_iter()
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

        arcbar.lock().unwrap().total += if first_iter {
            first_iter = false;
            chunk.len() + c2.len()
        } else {
            c2.len()
        };

        let abar = arcbar.clone();
        let chunk: Vec<_> = chunk
            .into_iter()
            .filter_map(|p| {
                if let Some((max_width, max_height)) = options.max_dimensions {
                    let img = ImageReader::open(&p.0)
                        .expect("cant open image file to read")
                        .with_guessed_format();
                    match img {
                        Err(_) => {
                            eprintln!("Failed to read image to check dimensions: {:?}", p.0);
                            return None;
                        }
                        Ok(img) => match img.into_dimensions() {
                            Err(e) => {
                                eprintln!("Failed to decode image dimensions: {}", e);
                                return None;
                            }
                            Ok((width, height)) => {
                                if width > max_width || height > max_height {
                                    if options.debug {
                                        eprintln!(
                                            "skipping image: {:?} with dimensions {}x{}",
                                            p.0, width, height
                                        );
                                    }
                                    return None;
                                }
                            }
                        },
                    };
                }
                if options.rescan {
                    return Some(p);
                }
                if db.is_indexed(&p.0, &p.1) {
                    db.unmark_file(&p.0);
                    abar.lock().unwrap().update(1).unwrap();
                    return None;
                }
                return Some(p);
            })
            .collect();

        let abar = arcbar.clone();
        let results: Vec<OcrResult> = chunk
            .par_iter()
            .map_init(
                || Ocr::new(&options.lang, options.debug).unwrap(),
                move |ocr, ele| {
                    if options.debug {
                        eprintln!("now working on {}", &ele.0.to_str().unwrap());
                    }
                    let res = ocr.scan(&ele.0);
                    abar.lock().unwrap().update(1).unwrap();
                    match res {
                        Ok(res) => Some(OcrResult {
                            path: ele.0.clone(),
                            metadata: ele.1.clone(),
                            contents: res,
                        }),
                        Err(e) => {
                            eprintln!("[Error] ocr: {}", e);
                            None
                        }
                    }
                },
            )
            .filter_map(|x| x)
            .collect();

        let count = db.save_results(results)?;
        if options.debug {
            eprintln!("Saved {count} to the db");
        }
    }

    let deleted = db.sweep_deletions();
    if options.debug {
        eprintln!("Deleted {deleted} stale entries");
    }

    Ok(())
}
