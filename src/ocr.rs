use anyhow::{anyhow, Result};
use camino::Utf8Path as Path;
use std::ffi::CString;

use leptess::tesseract::TessApi;
use leptonica_plumbing::{self, leptonica_sys};

pub struct Ocr {
    leptess: TessApi,
}

impl Ocr {
    pub fn new(lang: &str, debug: bool) -> Result<Self> {
        if lang.len() != 3 || lang.contains(['.', '/', '\\']) || !lang.is_ascii() {
            return Err(anyhow!("Invalid language code: {:?}", lang));
        }

        let mut leptess = TessApi::new(None, &lang.to_ascii_lowercase())?;

        if !debug {
            leptess
                .raw
                .set_variable(
                    leptess::Variable::DebugFile.as_cstr(),
                    &CString::new("/dev/null").unwrap(),
                )
                .unwrap();
            set_log_level(leptonica_sys::L_SEVERITY_ERROR);
        }

        Ok(Ocr { leptess })
    }
    pub fn scan(&mut self, img: &Path) -> Result<String> {
        let filename = CString::new(img.as_str()).expect("null in filename");
        let cpix = leptonica_plumbing::Pix::read_with_hint(
            &filename,
            leptonica_sys::L_JPEG_CONTINUE_WITH_BAD_DATA,
        )?;

        self.leptess.set_image(&leptess::leptonica::Pix {
            raw: cpix.to_ref_counted(),
        });

        Ok(self.leptess.get_utf8_text()?)
    }
}

fn set_log_level(level: u32) {
    unsafe {
        leptonica_sys::setMsgSeverity(level.try_into().unwrap());
    }
}
