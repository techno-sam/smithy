/*
* Smithy
* Copyright (C) 2025  Sam Wagenaar
* This program is free software: you can redistribute it and/or modify
* it under the terms of the GNU Affero General Public License as published by
* the Free Software Foundation, either version 3 of the License, or
* (at your option) any later version.
* This program is distributed in the hope that it will be useful,
* but WITHOUT ANY WARRANTY; without even the implied warranty of
* MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
* GNU Affero General Public License for more details.
* You should have received a copy of the GNU Affero General Public License
* along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

use std::{fs::{File, OpenOptions}, path::Path, time::SystemTime};

pub(crate) struct GuardedFile {
    file: File,
    known_mtime: SystemTime
}
impl GuardedFile {
    pub(crate) fn new<P: AsRef<Path>>(path: P, writable: bool) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(writable)
            .create(false)
            .open(path)?;

        let known_mtime = file.metadata()?.modified()?;

        Ok(Self { file, known_mtime })
    }

    pub(crate) fn get(&self) -> &File {
        &self.file
    }

    /// (changed, file)
    pub(crate) fn get_mut(&mut self) -> (bool, &mut File) {
        let now = SystemTime::now();

        let mtime = match self.file.metadata().and_then(|meta| meta.modified()) {
            Ok(mtime) => mtime,
            Err(_) => now
        };

        let changed = mtime > self.known_mtime;
        self.known_mtime = now;

        (changed, &mut self.file)
    }
}
