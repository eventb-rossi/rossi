//! Rebuild a Rodin `.zip` archive with our generated `.bcc` / `.bcm` files.
//!
//! Takes the source archive (everything Rodin knows) and a [`BuildResult`]
//! (everything we produced) and returns a fresh zip:
//!
//! * `.bum` / `.buc` and `.project` are copied byte-exact from the input.
//! * `.bcm` / `.bcc` directly inside a rebuilt project are **dropped** and
//!   replaced with ours (nested ones are not components and are kept).
//! * `.bpo` / `.bps` from the input are **replaced** by the generated
//!   obligations reconciled with them (see [`crate::pog::reconcile`]):
//!   unchanged obligations keep their stamps and status rows, changed
//!   ones are marked by a fresh stamp, and an unchanged component's
//!   files come out byte-identical.
//! * `.bpr` (proofs) are copied byte-exact — proofs are user data that
//!   must survive regeneration; proofs of vanished obligations simply
//!   become orphans.
//! * Everything else (iUML-B `.cd` / `.smd`, LaTeX exports, etc.) is copied
//!   as-is so the archive layout matches the original.
//!
//! The top-level directory inside the archive is preserved so the `.project`
//! descriptor's relative paths stay valid.

use std::io::{Read, Seek, Write};

use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::BuildResult;

/// Repackage `input_zip_bytes` with our generated build files (single project).
///
/// Convenience wrapper around [`repackage_zip_bytes_multi`] for an archive that
/// holds one project: the destination prefix is detected from the input's
/// entries (the first top-level directory) and `build_result` is dropped under
/// it. Byte-identical to the historical single-project behavior.
pub fn repackage_zip_bytes(
    input_zip_bytes: &[u8],
    build_result: &BuildResult,
) -> std::io::Result<Vec<u8>> {
    let reader = std::io::Cursor::new(input_zip_bytes);
    let mut archive = ZipArchive::new(reader).map_err(zip_to_io)?;
    // Detect the prefix and repack from the same parsed archive (one parse).
    let prefix = detect_top_level_prefix(&mut archive)?;
    repackage_archive(archive, std::iter::once((prefix.as_str(), build_result)))
}

/// Repackage `input_zip_bytes`, dropping each project's generated files under
/// its own archive directory.
///
/// A Rodin `.zip` may bundle several top-level projects (see
/// [`crate::project::discover_projects`]); `builds` pairs each project's archive
/// prefix (e.g. `"MyProject/"`, or `""` for a flat archive) with the
/// [`BuildResult`] to place under it. Returns a fresh zip's bytes:
///
/// * All entries from the input are copied byte-exact *except* the
///   `.bcm` / `.bcc` / `.bpo` / `.bps` files sitting directly inside a
///   rebuilt prefix (so each project's `.bum`/`.buc`/`.project`, its
///   `.bpr` proofs, any sibling-project directory — e.g. a source-only
///   dir with no components — and the derived files of nested
///   directories that discovery does not treat as components are all
///   preserved untouched).
/// * One entry per [`crate::ScFile`] is written at `format!("{prefix}{filename}")`,
///   with each `.bpo` / `.bps` pair first reconciled against the input's
///   entry of the same name so unchanged obligations keep their stamps and
///   statuses, and stale statuses then recomputed from the kept `.bpr`
///   proofs ([`crate::pog::status::update_statuses`]). Output entries
///   are keyed by prefix + filename, so the same component basename
///   appearing in several sub-projects never overwrites another.
///
/// `builds` is taken as an iterator of `(prefix, build_result)` borrows so
/// callers can pass `results.iter().map(...)` without materializing an adapter
/// `Vec` or cloning the prefixes.
pub fn repackage_zip_bytes_multi<'a>(
    input_zip_bytes: &[u8],
    builds: impl IntoIterator<Item = (&'a str, &'a BuildResult)>,
) -> std::io::Result<Vec<u8>> {
    let reader = std::io::Cursor::new(input_zip_bytes);
    let archive = ZipArchive::new(reader).map_err(zip_to_io)?;
    repackage_archive(archive, builds)
}

/// Copy `archive`'s kept entries and drop each project's generated files under
/// its prefix. Shared by [`repackage_zip_bytes`] and [`repackage_zip_bytes_multi`]
/// so neither re-parses the archive (the single-project wrapper detects its
/// prefix from the same handle it passes here).
fn repackage_archive<'a, R: Read + Seek>(
    mut archive: ZipArchive<R>,
    builds: impl IntoIterator<Item = (&'a str, &'a BuildResult)>,
) -> std::io::Result<Vec<u8>> {
    let builds: Vec<(&str, &BuildResult)> = builds.into_iter().collect();
    let archive_comment = archive.comment().to_vec();
    let mut out = std::io::Cursor::new(Vec::<u8>::new());
    let mut writer = ZipWriter::new(&mut out);
    writer
        .set_raw_comment(archive_comment.into_boxed_slice())
        .map_err(zip_to_io)?;
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // The previous contents of every file about to be regenerated, for
    // reconciling the generated files against (unreadable entries count
    // as absent), and the archive index of every kept `.bpr` proof —
    // decompressed only when a component's status update asks for it,
    // since proof files can reach hundreds of megabytes.
    let mut previous = std::collections::HashMap::new();
    let mut proof_index = std::collections::HashMap::new();

    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(zip_to_io)?;
        if entry.name().ends_with(".bpr") {
            proof_index.insert(entry.name().to_string(), i);
        }
    }

    // A generated file is replaced only when it sits directly inside a
    // project being rebuilt; derived files in nested directories are
    // not components (discovery is direct-child only) and nothing
    // would regenerate them, so they are preserved like any other
    // sibling data.
    let replaced = |name: &str| {
        (name.ends_with(".bcm")
            || name.ends_with(".bcc")
            || name.ends_with(".bpo")
            || name.ends_with(".bps"))
            && builds.iter().any(|(prefix, _)| {
                name.strip_prefix(prefix)
                    .is_some_and(|rest| !rest.contains('/'))
            })
    };
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(zip_to_io)?;
        if replaced(entry.name()) {
            let name = entry.name().to_string();
            let mut text = String::new();
            if entry.read_to_string(&mut text).is_ok() {
                previous.insert(name, text);
            }
            continue;
        }
        // raw_copy_file marks directory Unix modes as regular files.
        if entry.is_dir() {
            let name = entry.name().to_string();
            let mut options =
                SimpleFileOptions::default().unix_permissions(entry.unix_mode().unwrap_or(0o755));
            if let Some(last_modified) = entry.last_modified().filter(zip::DateTime::is_valid) {
                options = options.last_modified_time(last_modified);
            }
            let options = options
                .into_full_options()
                .with_file_comment(entry.comment());
            writer.add_directory(name, options).map_err(zip_to_io)?;
            continue;
        }
        writer.raw_copy_file(entry).map_err(zip_to_io)?;
    }

    for (prefix, build_result) in &builds {
        let mut files = build_result.files.clone();
        let synthesized = crate::pog::reconcile::reconcile_build_files(&mut files, |name| {
            previous.get(&format!("{prefix}{name}")).cloned()
        });
        crate::pog::status::update_statuses(&mut files, &synthesized, |name| {
            let index = *proof_index.get(&format!("{prefix}{name}"))?;
            let mut entry = archive.by_index(index).ok()?;
            let mut bytes = Vec::new();
            // A short read is deliberately tolerated: a proof file we
            // can see but not fully decompress must still count as
            // present, so the status update carries its rows verbatim
            // (the parse fails) instead of resetting them.
            let _ = entry.read_to_end(&mut bytes);
            Some(bytes)
        });
        for file in &files {
            let path = format!("{prefix}{}", file.filename);
            writer.start_file(&path, options).map_err(zip_to_io)?;
            writer.write_all(file.contents.as_bytes())?;
        }
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

    fn entry_snapshot(
        bytes: &[u8],
        name: &str,
    ) -> (
        zip::CompressionMethod,
        Option<zip::DateTime>,
        Option<u32>,
        String,
        bool,
        Vec<u8>,
    ) {
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let entry = archive.by_name(name).unwrap();
        let start = entry.data_start().unwrap() as usize;
        let end = start + entry.compressed_size() as usize;
        (
            entry.compression(),
            entry.last_modified(),
            entry.unix_mode(),
            entry.comment().to_string(),
            entry.is_dir(),
            bytes[start..end].to_vec(),
        )
    }

    #[test]
    fn retained_entries_keep_compressed_bytes_and_metadata() {
        let timestamp = zip::DateTime::from_date_and_time(2024, 2, 6, 12, 34, 56).unwrap();
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut cursor);
        writer
            .set_raw_comment(b"archive comment".to_vec().into_boxed_slice())
            .unwrap();
        let directory_options = SimpleFileOptions::default()
            .last_modified_time(timestamp)
            .unix_permissions(0o750)
            .into_full_options()
            .with_file_comment("directory comment");
        writer
            .add_directory("m/extras/", directory_options)
            .unwrap();
        let file_options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(timestamp)
            .unix_permissions(0o640)
            .into_full_options()
            .with_file_comment("file comment");
        writer
            .start_file("m/extras/notes.txt", file_options)
            .unwrap();
        writer.write_all(b"retained payload").unwrap();
        writer.finish().unwrap();
        let input = cursor.into_inner();

        let build_result = BuildResult {
            files: vec![],
            diagnostics: vec![],
        };
        let output = repackage_zip_bytes(&input, &build_result).unwrap();

        let input_archive = ZipArchive::new(std::io::Cursor::new(&input)).unwrap();
        let output_archive = ZipArchive::new(std::io::Cursor::new(&output)).unwrap();
        assert_eq!(output_archive.comment(), input_archive.comment());
        assert_eq!(
            entry_snapshot(&output, "m/extras/"),
            entry_snapshot(&input, "m/extras/")
        );
        assert_eq!(
            entry_snapshot(&output, "m/extras/notes.txt"),
            entry_snapshot(&input, "m/extras/notes.txt")
        );
    }

    /// A generated `.bpo` with one sequent, stamped `stamp` throughout.
    fn bpo(stamp: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.poFile org.eventb.core.poStamp="{stamp}">
<org.eventb.core.poPredicateSet name="ABSHYP" org.eventb.core.poStamp="{stamp}">
<org.eventb.core.poIdentifier name="x" org.eventb.core.type="ℤ"/>
</org.eventb.core.poPredicateSet>
<org.eventb.core.poSequent name="evt/inv1/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="{stamp}">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="M.bpo|org.eventb.core.poFile#M|org.eventb.core.poPredicateSet#ABSHYP"/>
<org.eventb.core.poPredicate name="SEQHYQ" org.eventb.core.predicate="x=0"/>
</org.eventb.core.poSequent>
</org.eventb.core.poFile>
"#
        )
    }

    /// A `.bps` with one row for the sequent in [`bpo`], stamped to
    /// match it (a mismatched stamp would mark the row stale and the
    /// status update would recompute it).
    fn bps(confidence: &str) -> String {
        bps_stamped(confidence, "0")
    }

    fn bps_stamped(confidence: &str, stamp: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
             <org.eventb.core.psFile>\n\
             <org.eventb.core.psStatus name=\"evt/inv1/INV\" org.eventb.core.confidence=\"{confidence}\" org.eventb.core.poStamp=\"{stamp}\" org.eventb.core.psManual=\"false\"/>\n\
             </org.eventb.core.psFile>\n"
        )
    }

    #[test]
    fn checked_files_keep_the_previous_declaration_order() {
        // A `.bcm` Rodin wrote lists the variables in its hash order;
        // the regenerated one lists them sorted and must come out in
        // Rodin's order, since Rodin compares children positionally.
        let previous = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
             <org.eventb.core.scMachineFile org.eventb.core.accurate=\"true\">\n\
                 <org.eventb.core.scVariable org.eventb.core.type=\"ℤ\" name=\"b\"/>\n\
                 <org.eventb.core.scVariable org.eventb.core.type=\"ℤ\" name=\"a\"/>\n\
             </org.eventb.core.scMachineFile>\n";
        let generated = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <org.eventb.core.scMachineFile org.eventb.core.accurate=\"true\">\n\
             <org.eventb.core.scVariable name=\"a\" org.eventb.core.type=\"ℤ\"/>\n\
             <org.eventb.core.scVariable name=\"b\" org.eventb.core.type=\"ℤ\"/>\n\
             </org.eventb.core.scMachineFile>\n";
        let input = make_zip(&[("m/M.bum", b"<m/>"), ("m/M.bcm", previous.as_bytes())]);
        let br = BuildResult {
            files: vec![ScFile {
                filename: "M.bcm".into(),
                contents: generated.into(),
                accurate: true,
            }],
            diagnostics: vec![],
        };

        let out = repackage_zip_bytes(&input, &br).unwrap();
        let bcm = String::from_utf8(read_entry(&out, "m/M.bcm")).unwrap();
        let b = bcm.find("name=\"b\"").unwrap();
        let a = bcm.find("name=\"a\"").unwrap();
        assert!(b < a, "previous order lost:\n{bcm}");
        // Rossi's own layout, not Rodin's, is what gets written.
        assert!(!bcm.contains("    <"));
    }

    #[test]
    fn replaces_checked_files_and_preserves_proofs() {
        // The old obligations are semantically identical to the newly
        // generated ones but carry non-zero stamps and a discharged
        // status; GONE is a component that no longer exists.
        let old_bpo = bpo("3");
        let old_bps = bps_stamped("1000", "3");
        let input = make_zip(&[
            ("m/.project", b"<project/>"),
            ("m/M.bum", b"<m/>"),
            ("m/C.buc", b"<c/>"),
            ("m/M.bcm", b"OLD"),
            ("m/C.bcc", b"OLD"),
            ("m/M.bpr", b"OLD-PROOF"),
            ("m/M.bpo", old_bpo.as_bytes()),
            ("m/M.bps", old_bps.as_bytes()),
            ("m/GONE.bpo", b"<gone/>"),
            ("m/GONE.bps", b"<gone/>"),
            ("m/GONE.bpr", b"GONE-PROOF"),
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
                ScFile {
                    filename: "M.bpo".into(),
                    contents: bpo("0"),
                    accurate: true,
                },
                ScFile {
                    filename: "M.bps".into(),
                    contents: bps("-99"),
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
        assert_eq!(read_entry(&out, "m/M.bcm"), b"NEW-BCM");
        assert_eq!(read_entry(&out, "m/C.bcc"), b"NEW-BCC");
        assert_eq!(read_entry(&out, "m/M.bum"), b"<m/>");

        // Proofs are copied byte-exact, even for vanished components.
        assert_eq!(read_entry(&out, "m/M.bpr"), b"OLD-PROOF");
        assert_eq!(read_entry(&out, "m/GONE.bpr"), b"GONE-PROOF");

        // The unchanged obligations come out byte-identical to the old
        // files: stamps and the discharged status survive the rebuild.
        assert_eq!(read_entry(&out, "m/M.bpo"), old_bpo.as_bytes());
        assert_eq!(read_entry(&out, "m/M.bps"), old_bps.as_bytes());

        // A vanished component's derived files are gone.
        assert!(!names.iter().any(|n| n == "m/GONE.bpo"));
        assert!(!names.iter().any(|n| n == "m/GONE.bps"));
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

    fn one_file(filename: &str, contents: &str) -> BuildResult {
        BuildResult {
            files: vec![ScFile {
                filename: filename.into(),
                contents: contents.into(),
                accurate: true,
            }],
            diagnostics: vec![],
        }
    }

    #[test]
    fn multi_project_keys_outputs_by_prefix_not_filename() {
        // Two sibling projects sharing the SAME component filename — the case
        // the old single-prefix repack collapsed into one entry. Each also
        // carries its own previous proof statuses.
        let bps_a = bps_stamped("777", "1");
        let bps_b = bps_stamped("888", "2");
        let input = make_zip(&[
            ("A/M0.bum", b"<a/>"),
            ("A/M0.bcm", b"OLD-A"),
            ("A/M0.bpo", bpo("1").as_bytes()),
            ("A/M0.bps", bps_a.as_bytes()),
            ("B/M0.bum", b"<b/>"),
            ("B/M0.bcm", b"OLD-B"),
            ("B/M0.bpo", bpo("2").as_bytes()),
            ("B/M0.bps", bps_b.as_bytes()),
        ]);
        let build = |bcm: &str| BuildResult {
            files: vec![
                ScFile {
                    filename: "M0.bcm".into(),
                    contents: bcm.into(),
                    accurate: true,
                },
                ScFile {
                    filename: "M0.bpo".into(),
                    contents: bpo("0"),
                    accurate: true,
                },
                ScFile {
                    filename: "M0.bps".into(),
                    contents: bps("-99"),
                    accurate: true,
                },
            ],
            diagnostics: vec![],
        };
        let a = build("NEW-A");
        let b = build("NEW-B");
        let out = repackage_zip_bytes_multi(&input, [("A/", &a), ("B/", &b)]).unwrap();

        // Each project's output lands under its own dir with its own bytes,
        // reconciled against its own previous state.
        assert_eq!(read_entry(&out, "A/M0.bcm"), b"NEW-A");
        assert_eq!(read_entry(&out, "B/M0.bcm"), b"NEW-B");
        assert_eq!(read_entry(&out, "A/M0.bum"), b"<a/>");
        assert_eq!(read_entry(&out, "B/M0.bum"), b"<b/>");
        assert_eq!(read_entry(&out, "A/M0.bpo"), bpo("1").as_bytes());
        assert_eq!(read_entry(&out, "B/M0.bpo"), bpo("2").as_bytes());
        assert_eq!(read_entry(&out, "A/M0.bps"), bps_a.as_bytes());
        assert_eq!(read_entry(&out, "B/M0.bps"), bps_b.as_bytes());
    }

    #[test]
    fn sibling_dir_without_components_is_preserved_and_gets_no_output() {
        // A components-free sibling dir precedes the real Event-B project dir.
        let input = make_zip(&[
            ("src/.project", b"<p>src</p>"),
            ("src/diagram.txt", b"<diagram/>"),
            ("model/.project", b"<p>model</p>"),
            ("model/M.bum", b"<m/>"),
            ("model/M.bcm", b"OLD"),
        ]);
        let evb = one_file("M.bcm", "NEW");
        let out = repackage_zip_bytes_multi(&input, [("model/", &evb)]).unwrap();
        let names = list(&out);

        // The source-only dir is copied through verbatim and receives no checked file.
        assert!(names.contains(&"src/.project".to_string()));
        assert!(names.contains(&"src/diagram.txt".to_string()));
        assert!(!names.iter().any(|n| n == "src/M.bcm"));
        // The real Event-B project gets the regenerated file in its own dir.
        assert_eq!(read_entry(&out, "model/M.bcm"), b"NEW");
    }

    #[test]
    fn nested_derived_files_are_preserved_not_deleted() {
        // A directory nested below another dir holds an externally
        // built
        // component. Discovery is direct-child only, so nothing
        // rebuilds it — repack must copy its derived files through
        // instead of deleting them with no replacement.
        let input = make_zip(&[
            ("P/M.bum", b"<m/>"),
            ("P/M.bcm", b"OLD"),
            ("Sub/Q/N.bum", b"<n/>"),
            ("Sub/Q/N.bcm", b"NESTED-BCM"),
            ("Sub/Q/N.bpo", b"NESTED-BPO"),
            ("Sub/Q/N.bps", b"NESTED-BPS"),
        ]);
        let br = one_file("M.bcm", "NEW");
        let out = repackage_zip_bytes_multi(&input, [("P/", &br)]).unwrap();
        assert_eq!(read_entry(&out, "P/M.bcm"), b"NEW");
        assert_eq!(read_entry(&out, "Sub/Q/N.bcm"), b"NESTED-BCM");
        assert_eq!(read_entry(&out, "Sub/Q/N.bpo"), b"NESTED-BPO");
        assert_eq!(read_entry(&out, "Sub/Q/N.bps"), b"NESTED-BPS");
    }

    #[test]
    fn single_project_wrapper_is_byte_identical_to_multi() {
        let input = make_zip(&[("m/M.bum", b"<m/>"), ("m/M.bcm", b"OLD")]);
        let br = one_file("M.bcm", "NEW");
        let via_wrapper = repackage_zip_bytes(&input, &br).unwrap();
        let via_multi = repackage_zip_bytes_multi(&input, [("m/", &br)]).unwrap();
        assert_eq!(via_wrapper, via_multi);
    }
}
