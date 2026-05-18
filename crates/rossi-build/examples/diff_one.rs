//! Diff our output against Rodin's for a single zip. Useful for seeing
//! where we fall short, file by file.
//!
//! Run: cargo run -p rossi-build --example diff_one -- <zip-path> [<file-filter>]

use std::io::Read;

use rossi_build::{Project, build};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: diff_one <zip> [filename-substring]")?;
    let filter = args.next();

    let mut project = Project::from_zip_file(&path)?;
    let data = std::fs::read(&path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&data))?;
    let mut rodin = std::collections::HashMap::new();
    for i in 0..archive.len() {
        let mut e = archive.by_index(i)?;
        let n = e.name().to_string();
        if n.ends_with(".bcc") || n.ends_with(".bcm") {
            let base = n
                .rsplit_once('/')
                .map(|(_, b)| b.to_string())
                .unwrap_or(n.clone());
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            rodin.insert(base, s);
        }
    }

    // Use Rodin's own project name so URIs line up.
    if let Some(name) = rodin.values().find_map(|x| {
        let i = x.find("org.eventb.core.source=\"/")?;
        let rest = &x[i + "org.eventb.core.source=\"/".len()..];
        let slash = rest.find('/')?;
        Some(rest[..slash].to_string())
    }) {
        project.name = name;
    }

    let result = build(&project);
    for f in &result.files {
        if let Some(substr) = &filter
            && !f.filename.contains(substr.as_str())
        {
            continue;
        }
        match rodin.get(&f.filename) {
            Some(r) => {
                let eq = r.trim_end() == f.contents.trim_end();
                println!("{}: {}", f.filename, if eq { "PASS" } else { "FAIL" });
                if !eq {
                    println!("--- ours ---\n{}", f.contents);
                    println!("--- rodin ---\n{}", r);
                }
            }
            None => println!("{}: NO REFERENCE", f.filename),
        }
    }
    for d in &result.diagnostics {
        eprintln!("{d}");
    }
    Ok(())
}
