use ariadne::{Cache, Source};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{fs, io};

/// Keeps track of files in memory.
/// Note that file data is leaked,
/// so this should only be used for a
/// static cache.
#[derive(Debug, Default, Clone)]
pub struct FileCache {
    files: Rc<RefCell<HashMap<PathBuf, &'static str>>>,
    sources: Rc<RefCell<HashMap<PathBuf, &'static Source<&'static str>>>>,
}

impl FileCache {
    /// Gets the specified file, retrieving
    /// it from the disk if it doesn't exist.
    pub fn get(&self, file: &Path) -> io::Result<&'static str> {
        let mut files = self.files.borrow_mut();
        let mut sources = self.sources.borrow_mut();

        if let Some(file) = files.get(file) {
            return Ok(*file);
        }

        let data: &'static str = fs::read_to_string(file)?.leak();
        let source: &'static Source<&'static str> = Box::leak(Box::new(Source::from(data)));

        files.insert(file.to_path_buf(), data);
        sources.insert(file.to_path_buf(), source);

        Ok(data)
    }

    /// Gets the specified file, retrieving
    /// it from the disk if it doesn't exist.
    pub fn get_source(&self, file: &Path) -> io::Result<&'static Source<&'static str>> {
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

impl Cache<Path> for FileCache {
    type Storage = &'static str;

    fn fetch(&mut self, path: &Path) -> Result<&Source<&'static str>, impl Debug> {
        self.get_source(path)
    }

    fn display<'a>(&self, path: &'a Path) -> Option<impl Display + 'a> {
        Some(Box::new(path.display()))
    }
}
