//! SVN dump file parser

use crate::dump_format::{DumpEntry, DumpFormat};
use anyhow::Result;
use std::io::BufRead;

/// Parse SVN dump file
pub fn parse_dump_file<R: BufRead>(reader: R) -> Result<DumpFormat> {
    let mut dump = DumpFormat::new();
    let mut current_entry: Option<DumpEntry> = None;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        if trimmed.starts_with("SVN-fs-dump-format-version:") {
            dump.format_version = trimmed.split(": ").nth(1).unwrap_or("3").to_string();
        } else if trimmed.starts_with("UUID:") {
            dump.uuid = trimmed.split(": ").nth(1).unwrap_or("").to_string();
        } else if trimmed.starts_with("Revision-number:") {
            if let Some(entry) = current_entry.take() {
                dump.entries.push(entry);
            }
            let rev_num: u64 = trimmed.split(": ").nth(1).unwrap_or("0").parse()?;
            current_entry = Some(DumpEntry::new(rev_num));
        }
    }

    if let Some(entry) = current_entry {
        dump.entries.push(entry);
    }

    Ok(dump)
}
