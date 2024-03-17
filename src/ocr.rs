use anyhow::{anyhow, Result};
use camino::Utf8Path as Path;
use std::ffi::CString;

use leptess::tesseract::TessApi;
use leptonica_plumbing::{self, leptonica_sys};

#[derive(Debug)]
pub struct Ocr {
    leptess: TessApi,
    scale: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binarization {
    Otsu = 0,
    LeptonicaOtsu = 1,
    Sauvola = 2,
}

impl Ocr {
    pub fn new(
        lang: &str,
        debug: bool,
        scale: Option<f32>,
        binarization: Option<Binarization>,
        psm: Option<i64>,
    ) -> Result<Self> {
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
        if let Some(binarization) = binarization {
            leptess
                .raw
                .set_variable(
                    &CString::new("thresholding_method").unwrap(),
                    &CString::new((binarization as u8).to_string()).unwrap(),
                )
                .unwrap();
        }
        if let Some(psm) = psm {
            leptess.raw.set_page_seg_mode(psm.try_into().unwrap());
        }

        leptess
            .raw
            .set_variable(
                leptess::Variable::TesseditPagesegMode.as_cstr(),
                &CString::new("11").unwrap(),
            )
            .unwrap();

        leptess
            .raw
            .set_variable(
                leptess::Variable::TesseditCharBlacklist.as_cstr(),
                &CString::new("|®»«®©").unwrap(),
            )
            .unwrap();

        Ok(Ocr { leptess, scale })
    }
    pub fn scan(&mut self, img: &Path) -> Result<String> {
        let filename = CString::new(img.as_str()).expect("null in filename");
        let mut cpix = leptonica_plumbing::Pix::read_with_hint(
            &filename,
            leptonica_sys::L_JPEG_CONTINUE_WITH_BAD_DATA,
        )?;

        if let Some(scale) = self.scale {
            cpix.scale_general(scale, scale)?;
        }

        self.leptess.set_image(&leptess::leptonica::Pix {
            raw: cpix.to_ref_counted(),
        });

        Ok(self.leptess.get_utf8_text()?.replace("\n\n", "\n"))
    }
}

fn set_log_level(level: u32) {
    unsafe {
        leptonica_sys::setMsgSeverity(level.try_into().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;
    use std::process::Command;
    use tempfile::NamedTempFile;
    use tempfile::TempPath;

    fn test_image() -> TempPath {
        let path = NamedTempFile::new().unwrap().into_temp_path();
        let result = Command::new("convert")
            .args(
                concat!(
                    "-background lightblue -fill white -font Liberation-Sans ",
                    "-size 300x70 -pointsize 24 -gravity east ",
                    "label:haystackhayneedle"
                )
                .split(' ')
                .chain(iter::once(
                    format!("png:{}", path.to_str().unwrap()).as_str(),
                )),
            )
            .status()
            .unwrap();
        assert!(result.success());
        path
    }
    #[test]
    #[ignore]
    fn scan() -> Result<()> {
        let mut ocr = Ocr::new("eng", true, None, None, Some(11)).unwrap();
        let image = test_image();
        let result = ocr.scan(Path::from_path(&image).unwrap()).unwrap();
        assert!(result.contains("needle"));
        Ok(())
    }
}
