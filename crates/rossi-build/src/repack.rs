//! Rebuild a Rodin `.zip` archive with our generated `.bcc` / `.bcm` files.
//!
//! Takes the source archive (everything Rodin knows) and a [`BuildResult`]
//! (everything we produced) and returns a fresh zip:
//!
//! * `.bum` / `.buc` and `.project` are copied byte-exact from the input.
//! * `.bcm` / `.bcc` from the input are **dropped** and replaced with ours.
//! * `.bpr` / `.bpo` / `.bps` (proof artifacts) are **dropped** — they
//!   reference checked content we just rebuilt and stale ones can confuse
//!   downstream tools.
//! * Everything else (iUML-B `.cd` / `.smd`, LaTeX exports, etc.) is copied
//!   as-is so the archive layout matches the original.
//!
//! The top-level directory inside the archive is preserved so the `.project`
//! descriptor's relative paths stay valid.

use std::io::{Read, Seek, Write};

use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::BuildResult;

/// Repackage `input_zip_bytes` with our generated build files.
///
/// Returns a fresh zip's bytes. The output archive contains:
///
/// * All entries from the input *except* `.bcm` / `.bcc` / `.bpr` / `.bpo` / `.bps`.
/// * One entry per [`crate::ScFile`] in `build_result.files`, placed under the
///   archive's top-level directory (derived from the input's entries).
pub fn repackage_zip_bytes(
    input_zip_bytes: &[u8],
    build_result: &BuildResult,
) -> std::io::Result<Vec<u8>> {
    let reader = std::io::Cursor::new(input_zip_bytes);
    let mut archive = ZipArchive::new(reader).map_err(zip_to_io)?;
    let prefix = detect_top_level_prefix(&mut archive)?;

    let mut out = std::io::Cursor::new(Vec::<u8>::new());
    let mut writer = ZipWriter::new(&mut out);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(zip_to_io)?;
        let name = entry.name().to_string();
        if !keep_input_entry(&name) {
            continue;
        }
        if entry.is_dir() {
            writer.add_directory(&name, options).map_err(zip_to_io)?;
            continue;
        }
        writer.start_file(&name, options).map_err(zip_to_io)?;
        std::io::copy(&mut entry, &mut writer)?;
    }

    for file in &build_result.files {
        let path = if prefix.is_empty() {
            file.filename.clone()
        } else {
            format!("{prefix}{}", file.filename)
        };
        writer.start_file(&path, options).map_err(zip_to_io)?;
        writer.write_all(file.contents.as_bytes())?;
    }

    writer.finish().map_err(zip_to_io)?;
    Ok(out.into_inner())
}

/// Convenience wrapper around [`repackage_zip_bytes`] that reads from a file.
pub fn repackage_zip_file<P: AsRef<std::path::Path>>(
    input_zip: P,
    build_result: &BuildResult,
) -> std::io::Result<Vec<u8>> {
    let data = std::fs::read(input_zip)?;
    repackage_zip_bytes(&data, build_result)
}

fn keep_input_entry(name: &str) -> bool {
    let drop = name.ends_with(".bcm")
        || name.ends_with(".bcc")
        || name.ends_with(".bpr")
        || name.ends_with(".bpo")
        || name.ends_with(".bps");
    !drop
}

/// Find the archive's top-level directory (everything up to and including the
/// first `/`). Returns `""` for flat archives.
fn detect_top_level_prefix<R: Read + Seek>(archive: &mut ZipArchive<R>) -> std::io::Result<String> {
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_to_io)?;
        let name = entry.name();
        if let Some(slash) = name.find('/') {
            return Ok(name[..=slash].to_string());
        }
    }
    Ok(String::new())
}

fn zip_to_io(e: zip::result::ZipError) -> std::io::Error {
    match e {
        zip::result::ZipError::Io(io) => io,
        other => std::io::Error::other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScFile;

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut w = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
        cursor.into_inner()
    }

    fn list(bytes: &[u8]) -> Vec<String> {
        let mut a = ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        (0..a.len())
            .map(|i| a.by_index(i).unwrap().name().to_string())
            .collect()
    }

    fn read_entry(bytes: &[u8], name: &str) -> Vec<u8> {
        let mut a = ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let mut e = a.by_name(name).unwrap();
        let mut v = Vec::new();
        e.read_to_end(&mut v).unwrap();
        v
    }

    #[test]
    fn drops_old_bcm_and_bcc_and_proofs_but_keeps_sources() {
        let input = make_zip(&[
            ("m/.project", b"<project/>"),
            ("m/M.bum", b"<m/>"),
            ("m/C.buc", b"<c/>"),
            ("m/M.bcm", b"OLD"),
            ("m/C.bcc", b"OLD"),
            ("m/M.bpr", b"OLD-PROOF"),
            ("m/M.bpo", b"OLD-PROOF"),
            ("m/M.bps", b"OLD-PROOF"),
            ("m/extras/notes.tex", b"% notes"),
        ]);
        let br = BuildResult {
            files: vec![
                ScFile {
                    filename: "M.bcm".into(),
                    contents: "NEW-BCM".into(),
                    accurate: true,
                },
                ScFile {
                    filename: "C.bcc".into(),
                    contents: "NEW-BCC".into(),
                    accurate: true,
                },
            ],
            diagnostics: vec![],
        };

        let out = repackage_zip_bytes(&input, &br).unwrap();
        let names = list(&out);

        assert!(names.contains(&"m/M.bum".to_string()));
        assert!(names.contains(&"m/C.buc".to_string()));
        assert!(names.contains(&"m/.project".to_string()));
        assert!(names.contains(&"m/extras/notes.tex".to_string()));
        assert!(names.contains(&"m/M.bcm".to_string()));
        assert!(names.contains(&"m/C.bcc".to_string()));
        assert!(!names.iter().any(|n| n.ends_with(".bpr")));
        assert!(!names.iter().any(|n| n.ends_with(".bpo")));
        assert!(!names.iter().any(|n| n.ends_with(".bps")));

        assert_eq!(read_entry(&out, "m/M.bcm"), b"NEW-BCM");
        assert_eq!(read_entry(&out, "m/C.bcc"), b"NEW-BCC");
        assert_eq!(read_entry(&out, "m/M.bum"), b"<m/>");
    }

    #[test]
    fn flat_archive_writes_files_at_root() {
        let input = make_zip(&[("M.bum", b"<m/>"), ("M.bcm", b"OLD")]);
        let br = BuildResult {
            files: vec![ScFile {
                filename: "M.bcm".into(),
                contents: "NEW".into(),
                accurate: true,
            }],
            diagnostics: vec![],
        };
        let out = repackage_zip_bytes(&input, &br).unwrap();
        let names = list(&out);
        assert!(names.contains(&"M.bum".to_string()));
        assert!(names.contains(&"M.bcm".to_string()));
        assert_eq!(read_entry(&out, "M.bcm"), b"NEW");
    }
}
