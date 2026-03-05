use ariadne::{Cache, Source};
use include_dir::{Dir, include_dir};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{fs, io};

static PROJECT_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/std");

/// Keeps track of files in memory.
/// Note that file data is leaked,
/// so this should only be used for a
/// static cache.
#[derive(Debug, Default, Clone)]
pub struct FileCache {
    files: RefCell<HashMap<PathBuf, &'static str>>,
    sources: RefCell<HashMap<PathBuf, &'static Source<&'static str>>>,
}

thread_local! {
    /// The global file cache.
    /// Any cached modified shouldn't be modified
    /// while the compiler is running.
    pub static FILE_CACHE: &'static FileCache = Box::leak(Box::new(FileCache::default()));
}

/// Returns the global file cache (thread-local).
pub fn file_cache() -> &'static FileCache {
    FILE_CACHE.with(|v| *v)
}

impl FileCache {
    /// Reads a file from the disk, handling module imports (e.g., "std/...")
    /// correctly.
    fn read_file(file: &Path) -> Result<&'static str, Box<dyn Error>> {
        // Non-module import (relative path).
        if file.starts_with("./") || file.starts_with("../") || file.is_absolute() {
            return fs::read_to_string(file)
                .map(|v| v.leak() as &'static str)
                .map_err(|e| Box::new(e) as Box<dyn Error>);
        }

        // Module imports need to be routed to locally stored files.
        const STD_PREFIX: &str = "std/";
        if let Ok(std_path) = file.strip_prefix(STD_PREFIX) {
            return PROJECT_DIR
                .get_file(std_path)
                .ok_or(io::Error::from(io::ErrorKind::NotFound))
                .map(|v| {
                    v.contents_utf8()
                        // This should never happen unless we mess up the stdlib.
                        .unwrap_or_else(|| panic!("File {} is not UTF-8!", file.display()))
                })
                .map_err(|e| Box::new(e) as Box<dyn Error>);
        }

        Err(format!(
            "File {} is not a valid module (use ./ or ../ to refer to local files)!",
            file.display()
        )
        .into())
    }

    /// Gets the specified file, retrieving
    /// it from the disk if it doesn't exist.
    pub fn get(&self, file: &Path) -> Result<&'static str, Box<dyn Error>> {
        let mut files = self.files.borrow_mut();
        let mut sources = self.sources.borrow_mut();

        if let Some(file) = files.get(file) {
            return Ok(*file);
        }

        let data: &'static str = Self::read_file(file)?;
        let source: &'static Source<&'static str> = Box::leak(Box::new(Source::from(data)));

        files.insert(file.to_path_buf(), data);
        sources.insert(file.to_path_buf(), source);

        Ok(data)
    }

    /// Gets the specified file, retrieving
    /// it from the disk if it doesn't exist.
    pub fn get_source(&self, file: &Path) -> Result<&'static Source<&'static str>, Box<dyn Error>> {
        let mut files = self.files.borrow_mut();
        let mut sources = self.sources.borrow_mut();

        if let Some(file) = sources.get(file) {
            return Ok(*file);
        }

        let data: &'static str = fs::read_to_string(file)?.leak();
        let source: &'static Source<&'static str> = Box::leak(Box::new(Source::from(data)));

        files.insert(file.to_path_buf(), data);
        sources.insert(file.to_path_buf(), source);

        Ok(source)
    }

    /// Checks if the specified file exists in the cache.
    /// This should be an absolute path.
    pub fn exists(&self, file: &Path) -> bool {
        self.files.borrow().contains_key(file)
    }
}

impl Cache<Path> for &FileCache {
    type Storage = &'static str;

    fn fetch(&mut self, path: &Path) -> Result<&Source<&'static str>, impl Debug> {
        self.get_source(path)
    }

    fn display<'a>(&self, path: &'a Path) -> Option<impl Display + 'a> {
        Some(Box::new(path.display()))
    }
}
