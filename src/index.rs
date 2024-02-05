use std::sync::{Arc, Mutex};
use std::{env, iter};

use anyhow::Result;
use camino::{Utf8Path as Path, Utf8PathBuf as PathBuf};
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
            let path = PathBuf::try_from(file.into_path()).unwrap();
            if let Some(ext) = path.extension() {
                if indexed_filetypes.contains(&ext) {
                    return Some(path);
                }
            }
            None
        });

    let it = if let Some(limit) = options.limit {
        Either::Left(it.take(limit))
    } else {
        Either::Right(it)
    };

    if options.cleanup {
        db.mark_for_deletion(Path::from_path(&env::current_dir().unwrap()).unwrap());
    }

    let arcbar = Arc::new(Mutex::new(BarBuilder::default().total(0).build().unwrap()));

    // the chunking starves the rayon pool but its fine
    let chunks = it.chunks(options.chunksize);
    let tup = chunks
        .into_iter()
        .map(|x| x.collect())
        .chain(iter::once(vec![]))
        .tuple_windows::<(_, _)>();

    let mut first_iter = true;
    for (c1, c2) in tup {
        let chunk: Vec<_> = c1
            .into_iter()
            .filter_map(move |file| match file.metadata() {
                Ok(metadata) => Some((file, metadata)),
                Err(e) => {
                    eprintln!("Error fetching metadata: {}", e);
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
            .filter(|p| {
                if !options.rescan && db.is_indexed(&p.0, &p.1) {
                    db.unmark_file(&p.0);
                    abar.lock().unwrap().update(1).unwrap();
                    return false;
                }
                if let Some((max_width, max_height)) = options.max_dimensions {
                    let img = ImageReader::open(&p.0).and_then(|img| img.with_guessed_format());
                    match img {
                        Err(_) => {
                            eprintln!("Failed to read image to check dimensions: {}", p.0);
                            return false;
                        }
                        Ok(img) => match img.into_dimensions() {
                            Err(e) => {
                                eprintln!(
                                    "Failed to decode image dimensions: {} Skipping: {}",
                                    e, p.0
                                );
                                return false;
                            }
                            Ok((width, height)) => {
                                if width > max_width || height > max_height {
                                    if options.debug {
                                        eprintln!(
                                            "skipping image: {} with dimensions {}x{}",
                                            p.0, width, height
                                        );
                                    }
                                    return false;
                                }
                            }
                        },
                    };
                }
                true
            })
            .collect();

        let abar = arcbar.clone();
        let results: Vec<OcrResult> = chunk
            .par_iter()
            .map_init(
                || Ocr::new(&options.lang, options.debug).unwrap(),
                move |ocr, ele| {
                    if options.debug {
                        eprintln!("now working on {}", &ele.0);
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
                            eprintln!("[Error] ocr: {} {}", e, &ele.0);
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
