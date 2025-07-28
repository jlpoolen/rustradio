use crate::block::Block;
use crate::{Tag, WorkIo};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct ResilientFileSink {
    path: PathBuf,
    writer: Arc<Mutex<Option<std::fs::File>>>,
}

impl ResilientFileSink {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            writer: Arc::new(Mutex::new(None)),
        }
    }

    fn try_open(&self) {
        let mut guard = self.writer.lock().unwrap();
        if guard.is_none() {
            if let Ok(file) = OpenOptions::new().write(true).open(&self.path) {
                *guard = Some(file);
            }
        }
    }
}

impl Block for ResilientFileSink {
    fn work(
        &mut self,
        _io: &mut WorkIo,
        input: &[crate::blocks::BlockInput],
        _output: &mut [crate::blocks::BlockOutput],
    ) -> crate::Result<()> {
        let buffer = input[0].buffer::<f32>();

        self.try_open();

        let mut guard = self.writer.lock().unwrap();
        if let Some(writer) = guard.as_mut() {
            let data = unsafe {
                std::slice::from_raw_parts(
                    buffer.as_ptr() as *const u8,
                    buffer.len() * std::mem::size_of::<f32>(),
                )
            };
            if let Err(e) = writer.write_all(data) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    eprintln!("⚠️ Broken pipe detected, dropping output until reader returns.");
                    *guard = None; // drop writer so we can reattempt later
                } else {
                    return Err(e.into());
                }
            }
        }

        Ok(())
    }

    fn input_tags(&mut self, _: usize) -> &mut Vec<Tag> {
        &mut vec![]
    }
}
