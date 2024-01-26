use anyhow::Result;
use std::{env, path::Path};

use leptess::LepTess;
use leptonica_sys;

pub struct Ocr {
    leptess: LepTess,
}

impl Ocr {
    pub fn new(lang: &str, debug: bool) -> Result<Self> {
        let mut leptess = LepTess::new(None, lang)?;

        env::set_var("OMP_THREAD_LIMIT", "1");

        if !debug {
            leptess
                .set_variable(leptess::Variable::DebugFile, "/dev/null")
                .unwrap();
            set_log_level(leptonica_sys::L_SEVERITY_ERROR);
        }

        Ok(Ocr { leptess })
    }
    pub fn scan(&mut self, img: &Path) -> Result<String> {
        self.leptess.set_image(img)?;
        Ok(self.leptess.get_utf8_text()?)
    }
}

fn set_log_level(level: u32) {
    unsafe {
        leptonica_sys::setMsgSeverity(level.try_into().unwrap());
    }
}
