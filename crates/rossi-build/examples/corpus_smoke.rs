//! Manual probe: unzip each model in the corpus and report, per
//! `.bcc`/`.bcm` file, whether we produce a byte-exact match of Rodin's.
//!
//! Run:
//!   cargo run -p rossi-build --example corpus_smoke -- <corpus-dir>
//!
//! Or set:
//!   EVENTB_CORPUS_DIR=/path/to/corpus

use std::io::Read;
use std::path::PathBuf;

use rossi_build::{Project, build};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = corpus_dir()?;
    let mut total_ctx = 0usize;
    let mut exact_ctx = 0usize;
    let mut total_mch = 0usize;
    let mut exact_mch = 0usize;
    let mut failed_projects = 0usize;

    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.ends_with(".zip") {
            continue;
        }

        // Load Rodin's own .bcc/.bcm from the zip for diffing. Key by basename
        // so we can match our output regardless of whether the zip uses a
        // project-name directory prefix.
        let data = std::fs::read(&path)?;
        let mut rodin_files = std::collections::HashMap::new();
        {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&data))?;
            for i in 0..archive.len() {
                let mut e = archive.by_index(i)?;
                let n = e.name().to_string();
                if n.ends_with(".bcc") || n.ends_with(".bcm") {
                    let basename = n.rsplit_once('/').map(|(_, b)| b).unwrap_or(&n).to_string();
                    let mut s = String::new();
                    e.read_to_string(&mut s)?;
                    rodin_files.insert(basename, s);
                }
            }
        }

        // Extract the project name from any `source="/PROJECT/..."` in
        // Rodin's .bcc/.bcm files. Falls back to the zip stem.
        let project_name = rodin_files
            .values()
            .find_map(|xml| {
                let marker = "org.eventb.core.source=\"/";
                let i = xml.find(marker)?;
                let start = i + marker.len();
                let slash = xml[start..].find('/')?;
                Some(xml[start..start + slash].to_string())
            })
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string()
            });

        let project = match Project::from_zip_bytes(project_name, &data) {
            Ok(p) => p,
            Err(e) => {
                failed_projects += 1;
                eprintln!("  [load-fail] {name}: {e}");
                continue;
            }
        };

        let result = build(&project);
        for f in &result.files {
            let is_bcc = f.filename.ends_with(".bcc");
            if is_bcc {
                total_ctx += 1;
            } else {
                total_mch += 1;
            }
            if let Some(rodin) = rodin_files.get(&f.filename)
                && rodin.trim_end() == f.contents.trim_end()
            {
                if is_bcc {
                    exact_ctx += 1;
                } else {
                    exact_mch += 1;
                }
            }
        }
    }

    println!(
        "contexts: {exact_ctx} / {total_ctx} byte-exact  ({} %)",
        (100 * exact_ctx).checked_div(total_ctx).unwrap_or(0)
    );
    println!(
        "machines: {exact_mch} / {total_mch} byte-exact  ({} %)",
        (100 * exact_mch).checked_div(total_mch).unwrap_or(0)
    );
    if failed_projects > 0 {
        eprintln!("(skipped {failed_projects} archives that failed to load)");
    }
    Ok(())
}

fn corpus_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(arg) = std::env::args().nth(1) {
        return Ok(PathBuf::from(arg));
    }
    if let Ok(path) = std::env::var("EVENTB_CORPUS_DIR") {
        return Ok(PathBuf::from(path));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "usage: cargo run -p rossi-build --example corpus_smoke -- <corpus-dir>\n\
         or set EVENTB_CORPUS_DIR=/path/to/corpus",
    )
    .into())
}
