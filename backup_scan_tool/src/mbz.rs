//! MBZ archive layer.
//!
//! Moodle MBZ files come in two physical formats (see lib/filestorage/mbz_packer.php):
//! - ZIP: random-access via the central directory. We never decompress the
//!   big media payloads — we only stream the XML entries.
//! - TAR.GZ: sequential only. We must read the whole compressed stream once,
//!   but we still skip past (discard) non-XML payloads without buffering them.
//!
//! Both variants expose the same name-and-bytes interface to the scanner.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

pub enum MbzArchive {
    Zip(ZipBacked),
    TarGz(TarGzBacked),
}

pub struct ZipBacked {
    path: PathBuf,
    archive: zip::ZipArchive<BufReader<File>>,
    names: Vec<String>,
}

pub struct TarGzBacked {
    path: PathBuf,
    names: Vec<String>,
    /// XML entry contents, eagerly captured during the single tar pass.
    xml: HashMap<String, Vec<u8>>,
}

impl MbzArchive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut f = File::open(path)
            .with_context(|| format!("opening MBZ {}", path.display()))?;
        let mut magic = [0u8; 4];
        let n = f
            .read(&mut magic)
            .with_context(|| format!("reading MBZ magic {}", path.display()))?;
        f.seek(io::SeekFrom::Start(0))
            .with_context(|| format!("rewinding {}", path.display()))?;

        if n >= 2 && magic[0] == 0x1f && magic[1] == 0x8b {
            // gzip — assume tar inside.
            return Ok(MbzArchive::TarGz(open_targz(path, f)?));
        }
        if n >= 4 && &magic[..2] == b"PK" {
            return Ok(MbzArchive::Zip(open_zip(path, f)?));
        }
        // Default to zip; we'll get a clear error from the zip reader if not.
        Ok(MbzArchive::Zip(open_zip(path, f)?))
    }

    pub fn path(&self) -> &Path {
        match self {
            MbzArchive::Zip(z) => &z.path,
            MbzArchive::TarGz(t) => &t.path,
        }
    }

    pub fn names(&self) -> &[String] {
        match self {
            MbzArchive::Zip(z) => &z.names,
            MbzArchive::TarGz(t) => &t.names,
        }
    }

    pub fn xml_names(&self) -> impl Iterator<Item = &str> {
        self.names()
            .iter()
            .filter(|n| n.ends_with(".xml") && !n.ends_with('/'))
            .map(|n| n.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        self.names().iter().any(|n| n == name)
    }

    pub fn read_to_vec(&mut self, name: &str) -> Result<Vec<u8>> {
        match self {
            MbzArchive::Zip(z) => {
                let mut entry = z
                    .archive
                    .by_name(name)
                    .with_context(|| format!("entry {} not found in {}", name, z.path.display()))?;
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf).with_context(|| {
                    format!("decompressing {} from {}", name, z.path.display())
                })?;
                Ok(buf)
            }
            MbzArchive::TarGz(t) => t
                .xml
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("entry {} not found (or not XML) in {}", name, t.path.display())),
        }
    }
}

fn open_zip(path: &Path, f: File) -> Result<ZipBacked> {
    let archive = zip::ZipArchive::new(BufReader::new(f))
        .with_context(|| format!("reading zip central directory {}", path.display()))?;
    let names = archive.file_names().map(|n| n.to_owned()).collect();
    Ok(ZipBacked {
        path: path.to_path_buf(),
        archive,
        names,
    })
}

fn open_targz(path: &Path, f: File) -> Result<TarGzBacked> {
    let gz = flate2::read::GzDecoder::new(BufReader::new(f));
    let mut tar = tar::Archive::new(gz);

    let mut names: Vec<String> = Vec::new();
    let mut xml: HashMap<String, Vec<u8>> = HashMap::new();

    let entries = tar
        .entries()
        .with_context(|| format!("reading tar entries from {}", path.display()))?;

    for entry in entries {
        let mut entry = entry
            .with_context(|| format!("iterating tar entries in {}", path.display()))?;
        let header = entry.header();
        if header.entry_type().is_dir() {
            continue;
        }
        let raw_path = entry
            .path()
            .with_context(|| format!("reading tar entry path in {}", path.display()))?;
        let name = raw_path.to_string_lossy().into_owned();
        // Normalise: strip leading "./" that some tar writers prepend.
        let name = name.strip_prefix("./").map(str::to_owned).unwrap_or(name);

        let is_xml = name.ends_with(".xml");
        if is_xml {
            let mut buf = Vec::with_capacity(header.size().unwrap_or(0) as usize);
            entry
                .read_to_end(&mut buf)
                .with_context(|| format!("reading {} from {}", name, path.display()))?;
            xml.insert(name.clone(), buf);
        } else {
            // Drain the entry's data so the tar reader can advance to the next
            // entry, but don't buffer the bytes — io::copy with sink discards them.
            io::copy(&mut entry, &mut io::sink()).with_context(|| {
                format!("skipping non-xml entry {} in {}", name, path.display())
            })?;
        }
        names.push(name);
    }

    Ok(TarGzBacked {
        path: path.to_path_buf(),
        names,
        xml,
    })
}
