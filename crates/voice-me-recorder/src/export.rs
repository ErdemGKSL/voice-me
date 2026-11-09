//! The zip the Colab notebook trains from:
//!
//! ```text
//! voice-me-dataset-<speaker>-<count>.zip
//!   metadata.csv    00001.wav|Cümle metni         (only recorded sentences)
//!   wavs/00001.wav  mono, 22 050 Hz, 16-bit
//!   manifest.json   {"format":1,"sample_rate":22050,"language":"tr",
//!                    "phoneme_type":"dizge","speaker":…,"count":…,"seconds":…}
//! ```
//!
//! `notebooks/Piper_Train_Dizge.ipynb` reads exactly this layout.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::audio::TARGET_RATE;
use crate::sentences::Sentence;
use crate::session::Session;

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub path: PathBuf,
    pub count: usize,
    pub seconds: f64,
    /// Recordings left out because they were not 22 050 Hz mono 16-bit.
    pub skipped: usize,
}

fn wav_seconds(bytes: &[u8]) -> Option<f64> {
    let reader = hound::WavReader::new(Cursor::new(bytes)).ok()?;
    let spec = reader.spec();
    (spec.channels == 1
        && spec.sample_rate == TARGET_RATE
        && spec.bits_per_sample == 16
        && spec.sample_format == hound::SampleFormat::Int)
        .then(|| f64::from(reader.duration()) / f64::from(spec.sample_rate))
}

/// Safe for a file name.
fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "speaker".to_string() } else { slug.to_string() }
}

/// Which zip is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// What the Colab notebook trains from.
    Training,
    /// The same files plus `resume.json`, named so it is not mistaken for
    /// the training set: [`import_zip`] on another PC continues from it.
    Backup { next_id: String },
}

/// Write the training zip of every sentence in `sentences` that has a
/// recording in `session`, into `out_dir`.
pub fn export_zip(
    session: &Session,
    sentences: &[Sentence],
    out_dir: &Path,
) -> Result<Summary, String> {
    write_zip(session, sentences, out_dir, Kind::Training)
}

/// Write the zip to move the work to another PC; see [`Kind::Backup`].
pub fn export_backup(
    session: &Session,
    sentences: &[Sentence],
    out_dir: &Path,
    next_id: &str,
) -> Result<Summary, String> {
    write_zip(session, sentences, out_dir, Kind::Backup { next_id: next_id.to_string() })
}

/// What [`import_zip`] brought in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub added: usize,
    /// Already recorded here, so left as they are.
    pub kept: usize,
    /// Not 22 050 Hz mono 16-bit, or not a sentence id.
    pub skipped: usize,
    /// The sentence the backup stopped at, if it says. An id, not a
    /// position: the order can differ between versions of the recorder.
    pub next_id: Option<String>,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
}

/// Merge the recordings in a training or backup zip into `session`. A
/// sentence recorded here is never overwritten. The speaker's name comes
/// along when this session has none.
pub fn import_zip(session: &Session, zip_path: &Path) -> Result<Imported, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("could not open the zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("this is not a zip: {e}"))?;
    let have = session.recorded();
    let mut imported = Imported { added: 0, kept: 0, skipped: 0, next_id: None };

    let text = |archive: &mut zip::ZipArchive<std::fs::File>, name: &str| {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut archive.by_name(name).ok()?, &mut out).ok()?;
        Some(out)
    };
    if let Some(manifest) = text(&mut archive, "manifest.json")
        && let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&manifest)
        && !session.has_speaker()
        && let Some(speaker) = manifest["speaker"].as_str()
    {
        session.set_speaker(speaker)?;
    }
    if let Some(resume) = text(&mut archive, "resume.json")
        && let Ok(resume) = serde_json::from_str::<serde_json::Value>(&resume)
    {
        imported.next_id = resume["next_id"].as_str().filter(|id| safe_id(id)).map(String::from);
    }

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        let Some(id) = name.strip_prefix("wavs/").and_then(|n| n.strip_suffix(".wav")) else {
            continue;
        };
        if !safe_id(id) {
            imported.skipped += 1;
            continue;
        }
        if have.contains(id) {
            imported.kept += 1;
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|e| e.to_string())?;
        if wav_seconds(&bytes).is_none() {
            imported.skipped += 1;
            continue;
        }
        session.save(id, &bytes)?;
        imported.added += 1;
    }
    Ok(imported)
}

fn write_zip(
    session: &Session,
    sentences: &[Sentence],
    out_dir: &Path,
    kind: Kind,
) -> Result<Summary, String> {
    let speaker = session.speaker();
    let mut metadata = String::new();
    let mut files = Vec::new();
    let mut seconds = 0.0;
    let mut skipped = 0;
    for sentence in sentences {
        let Some(bytes) = session.load(&sentence.id) else { continue };
        let Some(length) = wav_seconds(&bytes) else {
            skipped += 1;
            continue;
        };
        seconds += length;
        metadata.push_str(&format!("{}.wav|{}\n", sentence.id, sentence.text));
        files.push((sentence.id.clone(), bytes));
    }
    if files.is_empty() {
        return Err("there is nothing recorded to export yet".to_string());
    }

    std::fs::create_dir_all(out_dir).map_err(|e| format!("could not create the export folder: {e}"))?;
    let prefix = match kind {
        Kind::Training => "voice-me-dataset",
        Kind::Backup { .. } => "voice-me-backup",
    };
    let path = out_dir.join(format!("{prefix}-{}-{}.zip", slug(&speaker), files.len()));
    let manifest = serde_json::json!({
        "format": 1,
        "sample_rate": TARGET_RATE,
        "language": "tr",
        "phoneme_type": "dizge",
        "speaker": speaker,
        "count": files.len(),
        "seconds": (seconds * 10.0).round() / 10.0,
    });

    let file = std::fs::File::create(&path).map_err(|e| format!("could not create the zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let write = |zip: &mut zip::ZipWriter<std::fs::File>, name: &str, data: &[u8]| {
        zip.start_file(name, deflated)
            .and_then(|()| zip.write_all(data).map_err(Into::into))
            .map_err(|e| format!("could not write {name}: {e}"))
    };
    write(&mut zip, "manifest.json", manifest.to_string().as_bytes())?;
    write(&mut zip, "metadata.csv", metadata.as_bytes())?;
    if let Kind::Backup { next_id } = &kind {
        let resume = serde_json::json!({ "format": 1, "next_id": next_id });
        write(&mut zip, "resume.json", resume.to_string().as_bytes())?;
    }
    for (id, bytes) in &files {
        write(&mut zip, &format!("wavs/{id}.wav"), bytes)?;
    }
    zip.finish().map_err(|e| format!("could not finish the zip: {e}"))?;
    Ok(Summary { path, count: files.len(), seconds, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn wav(seconds: f32, rate: u32) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for n in 0..(seconds * rate as f32) as usize {
            writer.write_sample((n % 100) as i16).unwrap();
        }
        writer.finalize().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn only_recorded_sentences_are_exported_with_their_text() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::open(dir.path().join("s")).unwrap();
        session.set_speaker("Erdem Göksel").unwrap();
        session.save("2", &wav(1.0, TARGET_RATE)).unwrap();
        session.save("3", &wav(1.0, 16_000)).unwrap(); // wrong rate: skipped
        session.save("1", &wav(2.0, TARGET_RATE)).unwrap();
        let sentences = ["1", "2", "3", "4"]
            .map(|id| Sentence { id: id.into(), text: format!("metin {id}") })
            .to_vec();

        let summary = export_zip(&session, &sentences, &dir.path().join("out")).unwrap();
        assert_eq!((summary.count, summary.skipped), (2, 1));
        assert!((summary.seconds - 3.0).abs() < 0.01);
        assert_eq!(
            summary.path.file_name().unwrap().to_str().unwrap(),
            "voice-me-dataset-erdem-g-ksel-2.zip"
        );

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&summary.path).unwrap()).unwrap();
        let mut read = |name: &str| {
            let mut out = String::new();
            archive.by_name(name).unwrap().read_to_string(&mut out).unwrap();
            out
        };
        assert_eq!(read("metadata.csv"), "1.wav|metin 1\n2.wav|metin 2\n");
        let manifest: serde_json::Value = serde_json::from_str(&read("manifest.json")).unwrap();
        assert_eq!(manifest["phoneme_type"], "dizge");
        assert_eq!(manifest["count"], 2);
        assert!(archive.by_name("wavs/1.wav").is_ok() && archive.by_name("wavs/4.wav").is_err());
    }

    #[test]
    fn a_backup_carries_the_work_to_another_session() {
        let dir = tempfile::tempdir().unwrap();
        let here = Session::open(dir.path().join("here")).unwrap();
        here.set_speaker("erdem").unwrap();
        here.save("1", &wav(1.0, TARGET_RATE)).unwrap();
        here.save("2", &wav(1.0, TARGET_RATE)).unwrap();
        let sentences = ["1", "2"]
            .map(|id| Sentence { id: id.into(), text: format!("metin {id}") })
            .to_vec();
        let backup = export_backup(&here, &sentences, &dir.path().join("out"), "2").unwrap();
        assert!(backup.path.file_name().unwrap().to_str().unwrap().starts_with("voice-me-backup-"));

        // Another PC: it already has sentence 2 and no speaker name.
        let there = Session::open(dir.path().join("there")).unwrap();
        let mine = wav(0.5, TARGET_RATE);
        there.save("2", &mine).unwrap();
        let imported = import_zip(&there, &backup.path).unwrap();
        assert_eq!(
            imported,
            Imported { added: 1, kept: 1, skipped: 0, next_id: Some("2".into()) }
        );
        assert_eq!(there.load("2").unwrap(), mine, "an existing take is never overwritten");
        assert!(there.load("1").is_some());
        assert_eq!(there.speaker(), "erdem");

        // A training zip imports too, without a resume point.
        let training = export_zip(&here, &sentences, &dir.path().join("out")).unwrap();
        let third = Session::open(dir.path().join("third")).unwrap();
        let imported = import_zip(&third, &training.path).unwrap();
        assert_eq!((imported.added, imported.next_id), (2, None));
        assert!(import_zip(&third, &dir.path().join("missing.zip")).is_err());
    }

    #[test]
    fn nothing_recorded_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::open(dir.path().join("s")).unwrap();
        assert!(export_zip(&session, &[], dir.path()).is_err());
    }
}
